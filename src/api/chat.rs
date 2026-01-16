use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;
use std::sync::Arc;
use crate::ax_state::AppState;
use crate::models::context::ChatRequest;
use crate::models::schema::{DataSource, FullSemanticNode};
use crate::infra::db_external::DynamicPool;
use crate::infra::db_internal::{pg_row_to_json, mysql_row_to_json};
use sqlx::Row;
use regex::Regex;

pub async fn chat_query(State(state): State<Arc<AppState>>, Json(payload): Json<ChatRequest>) -> impl IntoResponse {
    let query_text = payload.query.trim();
    let fst = state.fst.read().await;

    // --- 1. 语义识别阶段 ---
    let mut target_metrics: Vec<FullSemanticNode> = Vec::new();
    let mut potential_dims: Vec<(FullSemanticNode, String)> = Vec::new();

    // 捕获日期 (YYYY-MM-DD)
    let date_regex = Regex::new(r"(\d{4}-\d{2}-\d{2})").unwrap();
    let captured_date = date_regex.captures(query_text).map(|cap| cap[1].to_string());

    for entry in fst.node_cache.iter() {
        let n = entry.value();
        // A. 指标识别
        if n.node_role == "METRIC" && (query_text.contains(&n.label) || n.alias_names.iter().any(|a| query_text.contains(a))) {
            target_metrics.push(n.clone());
        }
        // B. 维度初步捕获
        if n.node_role == "DIMENSION" {
            // 时间维度自动判定
            let is_time = n.node_key.to_lowercase().contains("date") || n.label.contains("时间") || n.label.contains("日期");
            if is_time && captured_date.is_some() {
                potential_dims.push((n.clone(), captured_date.clone().unwrap()));
            }
            // 码值实例匹配 (A-Box)
            let instances = sqlx::query("SELECT value_label, value_code FROM dimension_values WHERE dimension_node_id = $1")
                .bind(n.id).fetch_all(&state.db).await.unwrap_or_default();
            for r in instances {
                let lbl: String = r.get("value_label");
                if query_text.contains(&lbl) { potential_dims.push((n.clone(), r.get("value_code"))); }
            }
        }
    }

    if target_metrics.is_empty() { return Json(json!({"status": "fail", "answer": "未识别出指标，请尝试明确指标名称。"})).into_response(); }
    if target_metrics.len() > 1 {
        return Json(json!({
            "status": "ambiguous", 
            "answer": "发现多个相似指标，请确认意图", 
            "candidates": target_metrics.iter().map(|n| n.label.clone()).collect::<Vec<_>>()
        })).into_response();
    }
    let metric = &target_metrics[0];

    // --- 2. T-Box 语义消歧 (解决同名维度冲突) ---
    let mut final_filters = Vec::new();
    for (dim, val) in potential_dims {
        // 只有该指标明确支持的维度，才会被视为有效意图
        let is_valid = sqlx::query("SELECT 1 FROM metric_dimension_rels WHERE metric_node_id = $1 AND dimension_node_id = $2")
            .bind(metric.id).bind(dim.id)
            .fetch_optional(&state.db).await.unwrap_or(None).is_some();
        if is_valid {
            final_filters.push((dim, val));
        }
    }

    // --- 3. 物理 SQL 推理生成 ---
    // 聚合方式判定：优先看提问中是否有“平均”
    let agg = if query_text.contains("平均") { "AVG" } else if metric.default_agg == "NONE" { "NONE" } else { &metric.default_agg };
    
    // Select 子句处理：使用 sql_expression 替代 target_column
    let select_clause = if agg == "NONE" { 
        format!("{} as \"{}\"", metric.sql_expression, metric.label) 
    } else { 
        format!("{}({}) as \"{}\"", agg, metric.sql_expression, metric.label) 
    };

    let mut wheres = vec!["1=1".to_string()];
    let mut group_bys = Vec::new();

    // 组合维度过滤条件与分组
    for (dim, val) in &final_filters {
        wheres.push(format!("{} = '{}'", dim.sql_expression, val));
        group_bys.push(dim.sql_expression.clone());
    }

    // 注入业务隐含约束 (Union Constraints)
    // 指标层的约束
    for c in &metric.default_constraints.0 { wheres.push(format!("{} {} '{}'", c.column, c.operator, c.value)); }
    // 命中维度的约束
    for (dim, _) in &final_filters {
        for c in &dim.default_constraints.0 { wheres.push(format!("{} {} '{}'", c.column, c.operator, c.value)); }
    }

    let mut sql = format!("SELECT {} FROM {} WHERE {}", select_clause, metric.target_table, wheres.join(" AND "));
    if agg != "NONE" && !group_bys.is_empty() {
        sql.push_str(&format!(" GROUP BY {}", group_bys.join(", ")));
    }

    // --- 4. 动态数据源路由执行 ---
    let source_res: Result<DataSource, _> = sqlx::query_as("SELECT * FROM data_sources WHERE id = $1")
        .bind(&metric.source_id).fetch_one(&state.db).await;
    
    let source = match source_res {
        Ok(s) => s,
        Err(_) => return Json(json!({"status": "error", "message": "映射的数据源已丢失"})).into_response(),
    };

    let pool = state.pool_manager.get_or_create_pool(&source).await.unwrap();

    match &*pool {
        DynamicPool::Postgres(p) => {
            let rows = sqlx::query(&sql).fetch_all(p).await.unwrap_or_default();
            Json(json!({
                "status": "success", 
                "sql": sql, 
                "logic": format!("基于{}聚合", agg),
                "data": rows.iter().map(pg_row_to_json).collect::<Vec<serde_json::Value>>()
            })).into_response()
        },
        DynamicPool::MySql(p) => {
            let rows = sqlx::query(&sql.replace("$1", "?")).fetch_all(p).await.unwrap_or_default();
            Json(json!({
                "status": "success", 
                "sql": sql.replace("$1", "?"), 
                "logic": format!("基于{}聚合", agg),
                "data": rows.iter().map(mysql_row_to_json).collect::<Vec<serde_json::Value>>()
            })).into_response()
        }
    }
}