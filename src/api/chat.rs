use axum::{extract::State, response::IntoResponse, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::ax_state::AppState;
use crate::models::context::ChatRequest;
use crate::models::schema::DataSource; // 移除了 FullSemanticNode
use crate::infra::db_external::DynamicPool;
use crate::infra::db_internal::{pg_row_to_json, mysql_row_to_json};

pub async fn chat_query(State(state): State<Arc<AppState>>, Json(payload): Json<ChatRequest>) -> impl IntoResponse {
    let query_text = payload.query.trim();
    let fst = state.fst.read().await;

    let mut metrics = Vec::new();
    let mut dimensions = Vec::new();
    let mut filter_value = String::new();

    // 1. 语义识别
    for entry in fst.node_cache.iter() {
        let n = entry.value();
        let mut hit_label = String::new();
        if query_text.contains(&n.label) { hit_label = n.label.clone(); }
        else {
            for alias in &n.alias_names {
                if query_text.contains(alias) { hit_label = alias.clone(); break; }
            }
        }

        if !hit_label.is_empty() {
            if n.node_type == "METRIC" { metrics.push(n.clone()); }
            else { 
                dimensions.push(n.clone());
                filter_value = query_text.replace(&hit_label, "").trim().to_string();
            }
        }
    }

    if metrics.is_empty() && dimensions.is_empty() {
        return Json(json!({"status": "fail", "answer": "未识别业务语义"})).into_response();
    }

    let primary_node = if !metrics.is_empty() { &metrics[0] } else { &dimensions[0] };
    
    // 2. 逻辑算子与约束提取
    let (agg_func, select_clause) = if query_text.contains("总") || query_text.contains("合计") {
        ("SUM", format!("SUM({})", primary_node.target_column))
    } else {
        ("NONE", "*".to_string())
    };

    let mut where_conds = Vec::new();
    if !dimensions.is_empty() {
        where_conds.push(format!("{} = $1", dimensions[0].target_column));
    }
    
    for m in &metrics {
        for c in &m.default_constraints.0 {
            where_conds.push(format!("{} {} '{}'", c.column, c.operator, c.value));
        }
    }

    let where_clause = if where_conds.is_empty() { "1=1".to_string() } else { where_conds.join(" AND ") };
    let sql = format!("SELECT {} FROM {} WHERE {}", select_clause, primary_node.target_table, where_clause);

    // 3. 执行查询
    let source: DataSource = sqlx::query_as("SELECT id, db_type, connection_url, display_name FROM data_sources WHERE id = $1")
        .bind(&primary_node.source_id).fetch_one(&state.db).await.unwrap();

    let pool = state.pool_manager.get_or_create_pool(&source).await.unwrap();
    match &*pool {
        DynamicPool::Postgres(pg_pool) => {
            let rows = sqlx::query(&sql).bind(&filter_value).fetch_all(pg_pool).await.unwrap();
            let data: Vec<Value> = rows.iter().map(pg_row_to_json).collect();
            Json(json!({
                "status": "success", 
                "sql": sql, 
                "logic": agg_func, // 使用了 agg_func 变量，消除警告
                "data": data
            })).into_response()
        },
        DynamicPool::MySql(mysql_pool) => {
            let sql_mysql = sql.replace("$1", "?"); 
            let rows = sqlx::query(&sql_mysql).bind(&filter_value).fetch_all(mysql_pool).await.unwrap();
            let data: Vec<Value> = rows.iter().map(mysql_row_to_json).collect();
            Json(json!({
                "status": "success", 
                "sql": sql_mysql, 
                "logic": agg_func, // 使用了 agg_func 变量，消除警告
                "data": data
            })).into_response()
        }
    }
}