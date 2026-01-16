mod api;
mod core;
mod infra;
mod models;
mod service;

use axum::{routing::{get, post,delete}, Router};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer; 
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt}; 

use sqlx::Row;

use crate::api::chat::chat_query;
use crate::api::mapping::{
    list_mappings, register_data_source, save_mapping, list_data_sources, 
    get_metadata_tables, get_metadata_columns, sync_dimension_values, export_ontology_ttl,
    delete_mapping
};
use crate::core::fst_engine::FstEngine;
use crate::core::inference::SemanticInferenceEngine;
use crate::infra::db_external::PoolManager;
use crate::models::schema::FullSemanticNode;

pub mod ax_state {
    use super::*;
    pub struct AppState {
        pub db: sqlx::PgPool,
        pub fst: RwLock<FstEngine>,
        pub pool_manager: PoolManager,
        pub engine: RwLock<SemanticInferenceEngine>, // 【核心】将推理引擎单例化
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 加载配置与初始化内部数据库
    dotenvy::dotenv().ok();

    // --- 1. 初始化 tracing 日志 ---
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "sse_backend=debug,tower_http=debug".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("🔧 正在初始化 SSE 企业级语义服务器...");

    let db = infra::db_internal::init_db().await;

    // 2. 核心：启动时加载全量语义节点 (初始化 FST)
    // 这里的 SQL 必须与 mapping.rs 中的 list 逻辑保持高度一致
    let mappings_res = sqlx::query_as::<sqlx::Postgres, FullSemanticNode>(
        r#"
        SELECT n.id, n.node_key, n.label, n.node_role, d.source_id, d.target_table, d.sql_expression, 
               d.default_constraints, d.alias_names, d.default_agg, n.dataset_id,
               COALESCE(array_agg(r.dimension_node_id) FILTER (WHERE r.dimension_node_id IS NOT NULL), '{}') as supported_dimension_ids
        FROM ontology_nodes n 
        JOIN semantic_definitions d ON n.id = d.node_id
        LEFT JOIN metric_dimension_rels r ON n.id = r.metric_node_id
        GROUP BY n.id, n.node_key, n.label, n.node_role, d.source_id, d.target_table, d.sql_expression, d.default_constraints, d.alias_names, d.default_agg, n.dataset_id
        "#
    )
    .fetch_all(&db)
    .await;

    let nodes = match mappings_res {
        Ok(n) => {
            tracing::info!("✅ [Init] 成功加载 {} 个语义节点到内存索引", n.len());
            n
        },
        Err(e) => {
            tracing::error!("❌ [Init] 无法加载语义节点: {:?}", e);
            Vec::new()
        }
    };

    // 3. 构建 FST 引擎
    let fst_engine = FstEngine::build(&nodes)?;

    // 初始化推理引擎并同步业务词典
    let mut inference_engine = SemanticInferenceEngine::new();
    
    // 提取所有可能的业务词汇（标签、别名、码值）
    let mut words = nodes.iter().flat_map(|n| {
        let mut v = vec![n.label.clone()];
        v.extend(n.alias_names.clone());
        v
    }).collect::<Vec<String>>();

    // 提取 A-Box 码值
    let codes = sqlx::query("SELECT value_label FROM dimension_values").fetch_all(&db).await?;
    words.extend(codes.into_iter().map(|r| r.get::<String, _>(0)));
    
    inference_engine.refresh_custom_words(words);
    
    // 4. 初始化全局状态
    let state = Arc::new(ax_state::AppState {
        db,
        fst: RwLock::new(fst_engine),
        pool_manager: PoolManager::new(),
        engine: RwLock::new(inference_engine),
    });

    // 5. 配置中间件与路由
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // 语义建模接口
        .route("/api/mappings", get(list_mappings))
        .route("/api/mapping", post(save_mapping))
        .route("/api/mapping/{id}", delete(delete_mapping))
        .route("/api/ontology/export", get(export_ontology_ttl))
        
        // 元数据与同步
        .route("/api/metadata/tables", get(get_metadata_tables))
        .route("/api/metadata/columns", get(get_metadata_columns))
        .route("/api/sync-values/{id}", post(sync_dimension_values))
        
        // 数据源管理
        .route("/api/datasource", post(register_data_source))
        .route("/api/datasources", get(list_data_sources))
        
        // 问数对话 (核心)
        .route("/api/chat", post(chat_query))
        
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    // 6. 启动服务
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("🔥 SSE Enterprise Backend is running on http://{}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    
    Ok(())
}