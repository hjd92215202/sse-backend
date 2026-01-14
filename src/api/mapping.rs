use axum::{
    extract::State,
    Json,
    response::IntoResponse,
    http::StatusCode,
};
use std::sync::Arc;
use crate::ax_state::AppState;
use crate::models::schema::{CreateMappingRequest, SemanticMapping};
use crate::core::fst_engine::FstEngine;

/// 处理保存映射的请求
/// 逻辑：写入数据库 -> 重新查询所有映射 -> 重建 FST 索引 -> 替换全局状态
pub async fn save_mapping(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateMappingRequest>,
) -> impl IntoResponse {
    // 1. 持久化到 PostgreSQL
    let insert_result = sqlx::query_as::<_, SemanticMapping>(
        r#"
        INSERT INTO semantic_mappings (entity_key, entity_label, alias_names, target_table, target_column,source_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (entity_key) 
        DO UPDATE SET 
            entity_label = EXCLUDED.entity_label,
            alias_names = EXCLUDED.alias_names,
            target_table = EXCLUDED.target_table,
            target_column = EXCLUDED.target_column,
            source_id = EXCLUDED.source_id
        RETURNING id, entity_key, entity_label, alias_names, target_table, target_column,source_id
        "#
    )
    .bind(&payload.entity_key)
    .bind(&payload.entity_label)
    .bind(&payload.alias_names)
    .bind(&payload.target_table)
    .bind(&payload.target_column)
    .bind(&payload.source_id)
    .fetch_one(&state.db)
    .await;

    match insert_result {
        Ok(_) => {
            // 2. 触发 FST 引擎热更新
            if let Err(e) = refresh_fst_cache(&state).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("DB saved but FST failed: {}", e) })),
                ).into_response();
            }

            (
                StatusCode::CREATED,
                Json(serde_json::json!({ "status": "success", "message": "Mapping saved and FST reloaded" })),
            ).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response(),
    }
}

/// 获取所有映射列表（供前端配置页面展示）
pub async fn list_mappings(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let rows = sqlx::query_as::<_, SemanticMapping>(
        "SELECT id, entity_key, entity_label, alias_names, target_table, target_column ,source_id FROM semantic_mappings"
    )
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// 内部函数：从数据库重新加载并构建 FST
async fn refresh_fst_cache(state: &AppState) -> anyhow::Result<()> {
    // 从数据库获取最新全量数据
    let all_mappings = sqlx::query_as::<_, SemanticMapping>(
        "SELECT id, entity_key, entity_label, alias_names, target_table, target_column, source_id FROM semantic_mappings"
    )
    .fetch_all(&state.db)
    .await?;

    // 构建新的 FstEngine
    let new_engine = FstEngine::build(&all_mappings)?;

    // 获取写锁并替换内存索引
    let mut fst_guard = state.fst.write().await;
    *fst_guard = new_engine;
    
    println!("FST Engine 已经成功热重载，当前实体数量: {}", all_mappings.len());
    Ok(())
}

pub async fn register_data_source(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<crate::models::schema::CreateDataSourceRequest>,
) -> impl IntoResponse {
    let result = sqlx::query(
        "INSERT INTO data_sources (id, db_type, connection_url, display_name) VALUES ($1, $2, $3, $4)"
    )
    .bind(&payload.id)
    .bind(&payload.db_type)
    .bind(&payload.connection_url)
    .bind(&payload.display_name)
    .execute(&state.db)
    .await;

    match result {
        Ok(_) => (StatusCode::CREATED, "Data Source Registered").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn list_data_sources(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let rows = sqlx::query_as::<_, crate::models::schema::DataSource>(
        "SELECT id, db_type, connection_url, display_name FROM data_sources"
    )
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}