mod api;
mod core;
mod infra;
mod models;
mod service; 

use axum::{routing, Router}; 
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::api::mapping::{save_mapping, list_mappings};
use crate::api::chat::chat_query;

// 定义全局状态
pub mod ax_state {
    use super::*;
    pub struct AppState {
        pub db: sqlx::PgPool,
        pub fst: RwLock<crate::core::fst_engine::FstEngine>,
    }
}

#[tokio::main]
async fn main() {
    // 加载环境变量
    dotenvy::dotenv().ok();
    
    // 1. 初始化数据库
    let db = infra::db_internal::init_db().await;

    // 2. 预加载映射数据并构建 FST
    // 注意：这里需要确保你的数据库里已经有了 semantic_mappings 表，或者即使为空也能运行
    let mappings = sqlx::query_as::<_, models::schema::SemanticMapping>(
        "SELECT id, entity_key, entity_label, alias_names, target_table, target_column FROM semantic_mappings"
    )
    .fetch_all(&db)
    .await
    .unwrap_or_default(); // 如果查询失败返回空列表

    let fst_engine = core::fst_engine::FstEngine::build(&mappings).unwrap();
    let state = Arc::new(ax_state::AppState {
        db,
        fst: RwLock::new(fst_engine),
    });

    // 3. 路由设置
    let app = Router::new()
        // 获取所有映射列表 (GET)
        .route("/api/mappings", routing::get(list_mappings))
        // 保存或更新映射 (POST)
        .route("/api/mapping", routing::post(save_mapping))
        .route("/api/chat", routing::post(chat_query))
        .with_state(state);

    // 4. 启动服务
    let addr = "0.0.0.0:3000";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("🚀 SSE 后端启动成功，监听接口: {}", addr);
    
    axum::serve(listener, app).await.unwrap();
}