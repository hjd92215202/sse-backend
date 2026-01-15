use axum::{extract::State, response::IntoResponse, Json};
use serde_json::{json, Value};
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
    let mut target_metric: Option<FullSemanticNode> = None;
    let mut detected_filters: Vec<(FullSemanticNode, String)> = Vec::new();

    let date_regex = Regex::new(r"(\d{4}-\d{2}-\d{2})").unwrap();
    let captured_date = date_regex.captures(query_text).map(|cap| cap[1].to_string());

    for entry in fst.node_cache.iter() {
        let n = entry.value();
        
        if n.node_role == "METRIC" && (query_text.contains(&n.label) || n.alias_names.iter().any(|a| query_text.contains(a))) {
            target_metric = Some(n.clone());
        }
        
        if n.node_role == "DIMENSION" {
            let is_time_dim = n.node_key.to_lowercase().contains("date") 
                             || n.node_key.to_lowercase().contains("dt") 
                             || n.label.contains("日期") 
                             || n.label.contains("时间");
            
            if is_time_dim && captured_date.is_some() {
                detected_filters.push((n.clone(), captured_date.clone().unwrap()));
            }

            let rows = sqlx::query("SELECT value_label, value_code FROM dimension_values WHERE dimension_node_id = $1")
                .bind(n.id).fetch_all(&state.db).await.unwrap_or_default();
            
            for row in rows {
                let label: String = row.get("value_label");
                let code: String = row.get("value_code");
                if query_text.contains(&label) {
                    detected_filters.push((n.clone(), code));
                }
            }
        }
    }

    // --- 2. 意图合法性校验 ---
    let metric = match target_metric {
        Some(m) => m,
        None => return Json(json!({"status": "fail", "answer": "抱歉，未识别出您要查询的指标名称。"})).into_response()
    };

    for (dim, _) in &detected_filters {
        let is_valid = sqlx::query(
            "SELECT 1 FROM metric_dimension_rels WHERE metric_node_id = $1 AND dimension_node_id = $2"
        ).bind(metric.id).bind(dim.id).fetch_optional(&state.db).await.unwrap_or(None);

        if is_valid.is_none() {
            return Json(json!({
                "status": "ambiguous",
                "answer": format!("语义冲突：指标 '{}' 不支持按维度 '{}' 进行分析。", metric.label, dim.label)
            })).into_response();
        }
    }

    // --- 3. 动态 SQL 生成 ---
    // 修复点：使用配置好的默认聚合方式 (default_agg)
    let select_clause = if query_text.contains("平均") {
        format!("AVG({})", metric.target_column)
    } else if metric.default_agg == "NONE" {
        metric.target_column.clone()
    } else {
        format!("{}({})", metric.default_agg, metric.target_column)
    };

    let mut where_conds = vec!["1=1".to_string()];
    for (dim_node, val_code) in &detected_filters {
        where_conds.push(format!("{} = '{}'", dim_node.target_column, val_code));
    }

    // 注入隐含约束：指标 + 维度
    for c in &metric.default_constraints.0 {
        where_conds.push(format!("{} {} '{}'", c.column, c.operator, c.value));
    }
    for (dim_node, _) in &detected_filters {
        for c in &dim_node.default_constraints.0 {
            where_conds.push(format!("{} {} '{}'", c.column, c.operator, c.value));
        }
    }

    let sql = format!("SELECT {} as value FROM {} WHERE {}", select_clause, metric.target_table, where_conds.join(" AND "));

    // --- 4. 执行数据路由 ---
    let source: DataSource = sqlx::query_as("SELECT * FROM data_sources WHERE id = $1").bind(&metric.source_id).fetch_one(&state.db).await.unwrap();
    let pool = state.pool_manager.get_or_create_pool(&source).await.unwrap();

    match &*pool {
        DynamicPool::Postgres(p) => {
            let rows = sqlx::query(&sql).fetch_all(p).await.unwrap_or_default();
            let data: Vec<Value> = rows.iter().map(pg_row_to_json).collect();
            Json(json!({
                "status": "success", 
                "sql": sql, 
                "logic": metric.default_agg, 
                "data": data
            })).into_response()
        },
        DynamicPool::MySql(p) => {
            let rows = sqlx::query(&sql.replace("$1", "?")).fetch_all(p).await.unwrap_or_default();
            let data: Vec<Value> = rows.iter().map(mysql_row_to_json).collect();
            Json(json!({
                "status": "success", 
                "sql": sql, 
                "logic": metric.default_agg, 
                "data": data
            })).into_response()
        }
    }
}