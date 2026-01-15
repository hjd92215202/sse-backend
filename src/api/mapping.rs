use crate::ax_state::AppState;
use crate::core::fst_engine::FstEngine;
use crate::models::schema::{
    CreateDataSourceRequest, CreateNodeRequest, DataSource, FullSemanticNode,
};
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

pub async fn save_mapping(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateNodeRequest>,
) -> impl IntoResponse {
    let mut tx = state.db.begin().await.unwrap();

    let node_id = match sqlx::query!(
        "INSERT INTO ontology_nodes (node_key, label, node_type) VALUES ($1, $2, $3) 
         ON CONFLICT (node_key) DO UPDATE SET label = EXCLUDED.label, node_type = EXCLUDED.node_type RETURNING id",
        payload.node_key, payload.label, payload.node_type
    ).fetch_one(&mut *tx).await {
        Ok(rec) => rec.id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let constraints_json = serde_json::to_value(&payload.default_constraints).unwrap();
    let _ = sqlx::query!(
        "INSERT INTO semantic_definitions (node_id, source_id, target_table, target_column, default_constraints, alias_names)
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (node_id) DO UPDATE SET 
         source_id = EXCLUDED.source_id, target_table = EXCLUDED.target_table, 
         target_column = EXCLUDED.target_column, default_constraints = EXCLUDED.default_constraints, 
         alias_names = EXCLUDED.alias_names",
        node_id, payload.source_id, payload.target_table, payload.target_column, constraints_json, &payload.alias_names
    ).execute(&mut *tx).await;

    tx.commit().await.unwrap();
    let _ = refresh_fst_cache(&state).await;
    (StatusCode::CREATED, "Mapping Saved").into_response()
}

pub async fn list_mappings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rows = sqlx::query_as::<_, FullSemanticNode>(
        "SELECT n.id, n.node_key, n.label, n.node_type, d.source_id, d.target_table, d.target_column, d.default_constraints, d.alias_names 
         FROM ontology_nodes n JOIN semantic_definitions d ON n.id = d.node_id"
    ).fetch_all(&state.db).await;

    match rows {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn register_data_source(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateDataSourceRequest>,
) -> impl IntoResponse {
    let res = sqlx::query(
        r#"
    INSERT INTO data_sources (id, db_type, connection_url, display_name) 
    VALUES ($1, $2, $3, $4)
    ON CONFLICT (id) DO UPDATE SET 
        db_type = EXCLUDED.db_type,
        connection_url = EXCLUDED.connection_url,
        display_name = EXCLUDED.display_name
    "#,
    )
    .bind(&payload.id)
    .bind(&payload.db_type)
    .bind(&payload.connection_url)
    .bind(&payload.display_name)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (StatusCode::CREATED, "Source Registered").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn list_data_sources(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rows = sqlx::query_as::<_, DataSource>(
        "SELECT id, db_type, connection_url, display_name FROM data_sources",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn refresh_fst_cache(state: &AppState) -> anyhow::Result<()> {
    let nodes = sqlx::query_as::<_, FullSemanticNode>(
        "SELECT n.id, n.node_key, n.label, n.node_type, d.source_id, d.target_table, d.target_column, d.default_constraints, d.alias_names 
         FROM ontology_nodes n JOIN semantic_definitions d ON n.id = d.node_id"
    ).fetch_all(&state.db).await?;
    let new_engine = FstEngine::build(&nodes)?;
    let mut guard = state.fst.write().await;
    *guard = new_engine;
    Ok(())
}
