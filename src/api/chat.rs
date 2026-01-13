use axum::{extract::State, response::IntoResponse, Json};
use serde_json::{json, Value};
use std::sync::Arc;

// 导入项目内部依赖
use crate::ax_state::AppState;
use crate::models::context::ChatRequest;
use crate::models::schema::{DataSource, SemanticMapping};
use crate::infra::db_external::DynamicPool;
use crate::infra::db_internal::{pg_row_to_json, mysql_row_to_json};

pub async fn chat_query(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ChatRequest>,
) -> impl IntoResponse {
    let query_text = payload.query.trim();

    // --- 1. 语义匹配环节 (Semantic Inference) ---
    let fst = state.fst.read().await;
    let mut matched_mapping: Option<SemanticMapping> = None;
    let mut filter_value = String::new();

    // 扫描 FST 缓存寻找匹配的实体或别名
    for entry in fst.mapping_cache.iter() {
        let m = entry.value();
        // 匹配主标签
        if query_text.contains(&m.entity_label) {
            matched_mapping = Some(m.clone());
            filter_value = query_text.replace(&m.entity_label, "").trim().to_string();
            break;
        }
        // 匹配别名
        let mut found_alias = false;
        for alias in &m.alias_names {
            if query_text.contains(alias) {
                matched_mapping = Some(m.clone());
                filter_value = query_text.replace(alias, "").trim().to_string();
                found_alias = true;
                break;
            }
        }
        if found_alias { break; }
    }

    let mapping = match matched_mapping {
        Some(m) => m,
        None => return Json(json!({ 
            "status": "fail", 
            "answer": "未识别出业务维度。请尝试输入关键词，如：结算平台、运营平台。" 
        })).into_response(),
    };

    // --- 2. 数据源路由 (DataSource Routing) ---
    let source_res = sqlx::query_as::<_, DataSource>(
        "SELECT id, db_type, connection_url, display_name FROM data_sources WHERE id = $1"
    )
    .bind(&mapping.source_id)
    .fetch_optional(&state.db)
    .await;

    let source = match source_res {
        Ok(Some(s)) => s,
        Ok(None) => return Json(json!({ "error": format!("数据源配置丢失: {}", mapping.source_id) })).into_response(),
        Err(e) => return Json(json!({ "error": format!("库查询失败: {}", e) })).into_response(),
    };

    // --- 3. 获取动态连接池 (Dynamic Pool Access) ---
    let pool = match state.pool_manager.get_or_create_pool(&source).await {
        Ok(p) => p,
        Err(e) => return Json(json!({ "error": format!("外部库连接失败: {}", e) })).into_response(),
    };

    // --- 4. 物理 SQL 执行 (结构对称化) ---
    match &*pool {
        DynamicPool::Postgres(pg_pool) => {
            // Postgres 语法：使用 $1 占位符
            let sql = format!(
                "SELECT * FROM {} WHERE {} = $1 LIMIT 10",
                mapping.target_table, mapping.target_column
            );

            let rows_result: Result<Vec<sqlx::postgres::PgRow>, sqlx::Error> = sqlx::query(&sql)
                .bind(&filter_value)
                .fetch_all(pg_pool)
                .await;

            match rows_result {
                Ok(rows) => {
                    let data: Vec<Value> = rows.iter().map(pg_row_to_json).collect();
                    // 统一返回格式
                    Json(json!({
                        "status": "success",
                        "db_type": "postgres",
                        "matched_entity": mapping.entity_label,
                        "query_info": {
                            "source": source.display_name,
                            "table": mapping.target_table,
                            "filter": filter_value
                        },
                        "data": data
                    })).into_response()
                },
                Err(e) => Json(json!({ "error": format!("Postgres 执行错误: {}", e) })).into_response(),
            }
        },
        DynamicPool::MySql(mysql_pool) => {
            // MySQL 语法：使用 ? 占位符
            let sql = format!(
                "SELECT * FROM {} WHERE {} = ? LIMIT 10",
                mapping.target_table, mapping.target_column
            );

            let rows_result: Result<Vec<sqlx::mysql::MySqlRow>, sqlx::Error> = sqlx::query(&sql)
                .bind(&filter_value)
                .fetch_all(mysql_pool)
                .await;

            match rows_result {
                Ok(rows) => {
                    let data: Vec<Value> = rows.iter().map(mysql_row_to_json).collect();
                    // 统一返回格式（与 Postgres 完全对齐）
                    Json(json!({
                        "status": "success",
                        "db_type": "mysql",
                        "matched_entity": mapping.entity_label,
                        "query_info": {
                            "source": source.display_name,
                            "table": mapping.target_table,
                            "filter": filter_value
                        },
                        "data": data
                    })).into_response()
                },
                Err(e) => Json(json!({ "error": format!("MySQL 执行错误: {}", e) })).into_response(),
            }
        }
    }
}