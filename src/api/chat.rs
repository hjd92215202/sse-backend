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

    // --- 1. 语义识别阶段 (FST + A-Box Search) ---
    let mut target_metrics: Vec<FullSemanticNode> = Vec::new();
    let mut detected_filters: Vec<(FullSemanticNode, String)> = Vec::new();

    // A. 时间正则捕获
    let date_regex = Regex::new(r"(\d{4}-\d{2}-\d{2})").unwrap();
    let captured_date = date_regex.captures(query_text).map(|cap| cap[1].to_string());

    for entry in fst.node_cache.iter() {
        let n = entry.value();
        
        // 识别指标名 (主名 + 别名)
        if n.node_role == "METRIC" && (query_text.contains(&n.label) || n.alias_names.iter().any(|a| query_text.contains(a))) {
            target_metrics.push(n.clone());
        }
        
        // 识别维度与实例值
        if n.node_role == "DIMENSION" {
            // 时间维度自动映射逻辑
            let is_time = n.node_key.to_lowercase().contains("date") || n.label.contains("时间") || n.label.contains("日期");
            if is_time && captured_date.is_some() {
                detected_filters.push((n.clone(), captured_date.clone().unwrap()));
            }

            // 搜索 A-Box 实例库
            let instances = sqlx::query("SELECT value_label, value_code FROM dimension_values WHERE dimension_node_id = $1")
                .bind(n.id).fetch_all(&state.db).await.unwrap_or_default();
            for inst in instances {
                let label: String = inst.get("value_label");
                let code: String = inst.get("value_code");
                if query_text.contains(&label) {
                    detected_filters.push((n.clone(), code));
                }
            }
        }
    }

    // --- 2. 冲突处理与意图对齐 ---
    if target_metrics.is_empty() {
        return Json(json!({"status": "fail", "answer": "未识别到指标，请尝试输入具体指标名称（如：收益额）。"})).into_response();
    }
    if target_metrics.len() > 1 {
        return Json(json!({
            "status": "ambiguous", 
            "answer": "发现多个相似指标，请确认您的意图", 
            "candidates": target_metrics.iter().map(|n| n.label.clone()).collect::<Vec<_>>()
        })).into_response();
    }

    let metric = &target_metrics[0];

    // --- 3. 语义连通性验证 (T-Box Check) ---
    for (dim, _) in &detected_filters {
        let is_valid = sqlx::query("SELECT 1 FROM metric_dimension_rels WHERE metric_node_id = $1 AND dimension_node_id = $2")
            .bind(metric.id).bind(dim.id).fetch_optional(&state.db).await.unwrap_or(None);
        if is_valid.is_none() {
            return Json(json!({
                "status": "ambiguous",
                "answer": format!("业务语义警告：指标 '{}' 并不支持按 '{}' 维度分析，查询结果可能无意义。", metric.label, dim.label)
            })).into_response();
        }
    }

    // --- 4. 逻辑计划 -> 物理 SQL 组装 ---
    // 识别提问中的显式聚合意图，否则使用本体定义的默认值
    let agg = if query_text.contains("平均") { 
        "AVG" 
    } else if metric.default_agg == "NONE" {
        "NONE"
    } else {
        &metric.default_agg
    };

    let select_clause = if agg == "NONE" { 
        metric.target_column.clone() 
    } else { 
        format!("{}({})", agg, metric.target_column) 
    };

    let mut where_conds = vec!["1=1".to_string()];
    
    // 合并识别到的物理过滤值
    for (dim_node, val) in &detected_filters {
        where_conds.push(format!("{} = '{}'", dim_node.target_column, val));
    }

    // 注入隐含业务约束：合并 [指标层] + [所有匹配到的维度层]
    for c in &metric.default_constraints.0 { 
        where_conds.push(format!("{} {} '{}'", c.column, c.operator, c.value)); 
    }
    for (dim, _) in &detected_filters {
        for c in &dim.default_constraints.0 { 
            where_conds.push(format!("{} {} '{}'", c.column, c.operator, c.value)); 
        }
    }

    let sql = format!("SELECT {} as value FROM {} WHERE {}", select_clause, metric.target_table, where_conds.join(" AND "));

    // --- 5. 跨库路由与执行 ---
    let source_res: Result<DataSource, _> = sqlx::query_as("SELECT * FROM data_sources WHERE id = $1")
        .bind(&metric.source_id).fetch_one(&state.db).await;
    
    let source = match source_res {
        Ok(s) => s,
        Err(_) => return Json(json!({"status": "error", "message": "无法找到映射的数据源"})).into_response(),
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