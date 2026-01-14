mod api;
mod core;
mod infra;
mod models;
mod service;

use axum::{
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};

use crate::api::chat::chat_query;
use crate::api::mapping::{list_mappings, register_data_source, save_mapping};
use crate::core::fst_engine::FstEngine;
use crate::infra::db_external::PoolManager;
use crate::models::schema::SemanticMapping;

// --- 全局应用状态定义 ---
pub mod ax_state {
    use super::*;
    pub struct AppState {
        // SSE 系统自用的数据库连接池 (存储映射、数据源配置)
        pub db: sqlx::PgPool,
        // 语义推断引擎：使用 RwLock 保证多读一写的热更新性能
        pub fst: RwLock<FstEngine>,
        // 外部数据源动态连接池管理器
        pub pool_manager: PoolManager,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 加载环境变量 (.env 文件)
    dotenvy::dotenv().ok();
    println!("🔧 正在启动 SSE 语义自我进化平台后端...");

    // 2. 初始化 SSE 内部系统数据库 (PostgreSQL)
    let db = infra::db_internal::init_db().await;
    println!("✅ 系统数据库连接成功");

    // 3. 预加载语义映射数据并构建初始 FST 引擎
    // 如果表中没有数据，build 会创建一个空的索引
    let mappings = sqlx::query_as::<_, SemanticMapping>(
        "SELECT id, entity_key, entity_label, alias_names, target_table, target_column, source_id FROM semantic_mappings"
    )
    .fetch_all(&db)
    .await
    .unwrap_or_else(|e| {
        eprintln!("⚠️ 警告：无法从数据库加载映射数据: {}", e);
        vec![]
    });

    let fst_engine = FstEngine::build(&mappings)?;
    println!("🧠 语义 FST 引擎初始化完成，已加载 {} 条实体", mappings.len());

    // 4. 初始化应用全局状态
    let state = Arc::new(ax_state::AppState {
        db,
        fst: RwLock::new(fst_engine),
        pool_manager: PoolManager::new(),
    });

    // 5. 配置跨域资源共享 (CORS) - 方便前端 Vue 项目调用
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // 6. 路由配置
    let app = Router::new()
        // --- 语义映射与数据源管理接口 (管理端) ---
        .route("/api/mappings", get(list_mappings))
        .route("/api/mapping", post(save_mapping))
        .route("/api/datasource", post(register_data_source))
        .route("/api/datasources", get(api::mapping::list_data_sources))
        
        // --- 问数对话核心接口 (业务端) ---
        .route("/api/chat", post(chat_query))
        
        // 注入全局状态与中间件
        .with_state(state)
        .layer(cors);

    // 7. 启动 HTTP 服务
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("🚀 SSE Backend 运行在 http://{}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}