mod api;
mod core;
mod infra;
mod models;
mod service;

use axum::{routing::{get, post}, Router};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};

use crate::api::chat::chat_query;
use crate::api::mapping::{list_mappings, register_data_source, save_mapping, list_data_sources};
use crate::core::fst_engine::FstEngine;
use crate::infra::db_external::PoolManager;
use crate::models::schema::FullSemanticNode;

pub mod ax_state {
    use super::*;
    pub struct AppState {
        pub db: sqlx::PgPool,
        pub fst: RwLock<FstEngine>,
        pub pool_manager: PoolManager,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let db = infra::db_internal::init_db().await;

    // 显式标注类型 FullSemanticNode，解决报错
    let mappings = sqlx::query_as::<_, FullSemanticNode>(
        "SELECT n.id, n.node_key, n.label, n.node_type, d.source_id, d.target_table, d.target_column, d.default_constraints, d.alias_names 
         FROM ontology_nodes n JOIN semantic_definitions d ON n.id = d.node_id"
    )
    .fetch_all(&db)
    .await
    .unwrap_or_default();

    let fst_engine = FstEngine::build(&mappings)?;
    let state = Arc::new(ax_state::AppState {
        db,
        fst: RwLock::new(fst_engine),
        pool_manager: PoolManager::new(),
    });

    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);

    let app = Router::new()
        .route("/api/mappings", get(list_mappings))
        .route("/api/mapping", post(save_mapping))
        .route("/api/ontology/export", get(crate::api::mapping::export_ontology_ttl))
        .route("/api/datasource", post(register_data_source))
        .route("/api/datasources", get(list_data_sources)) // 新增接口
        .route("/api/chat", post(chat_query))
        .with_state(state)
        .layer(cors);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    println!("🚀 SSE Backend 运行在 http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}