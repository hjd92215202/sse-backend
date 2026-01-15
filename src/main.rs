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

// 导入所有业务接口
use crate::api::chat::chat_query;
use crate::api::mapping::{
    list_mappings, register_data_source, save_mapping, list_data_sources, 
    get_metadata_tables, get_metadata_columns, sync_dimension_values, export_ontology_ttl
};
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

    // 初始化加载 FST 索引
    let mappings = sqlx::query_as::<_, FullSemanticNode>(
        "SELECT n.id, n.node_key, n.label, n.node_role, d.source_id, d.target_table, d.target_column, d.default_constraints, d.alias_names 
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
        // 本体映射管理
        .route("/api/mappings", get(list_mappings))
        .route("/api/mapping", post(save_mapping))
        .route("/api/ontology/export", get(export_ontology_ttl)) // 注册下载接口
        
        // 外部元数据探测
        .route("/api/metadata/tables", get(get_metadata_tables))
        .route("/api/metadata/columns", get(get_metadata_columns))
        
        // A-Box 维度实例同步
        .route("/api/sync-values/{id}", post(sync_dimension_values))
        
        // 数据源管理
        .route("/api/datasource", post(register_data_source))
        .route("/api/datasources", get(list_data_sources))
        
        // 语义问数对话核心
        .route("/api/chat", post(chat_query))
        
        .with_state(state)
        .layer(cors);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    println!("🚀 SSE Enterprise Backend 启动: http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}