use axum::{extract::State, Json, response::IntoResponse};
use std::sync::Arc;
use crate::ax_state::AppState;
use crate::models::context::ChatRequest;
use serde_json::json;
use crate::infra::utils::pg_row_to_json;

pub async fn chat_query(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ChatRequest>,
) -> impl IntoResponse {
    let query_text = payload.query.trim();

    // 1. 调用 FST 引擎进行实体匹配
    // MVP 简化逻辑：检查查询字符串中是否包含已知的实体标签
    let fst = state.fst.read().await;
    
    // 寻找匹配的映射逻辑
    let mut matched_mapping = None;
    let mut filter_value = String::new();

    // 这里的逻辑在实际生产中会更复杂（NLP分词），MVP 采用包含判定：
    // 如果用户说 "分润平台 A公司"，匹配到 "分润平台" 实体
    for entry in fst.mapping_cache.iter() {
        let m = entry.value();
        if query_text.contains(&m.entity_label) {
            matched_mapping = Some(m.clone());
            // 提取过滤值：简单去掉实体标签作为过滤值
            filter_value = query_text.replace(&m.entity_label, "").trim().to_string();
            break;
        }
    }

    let mapping = match matched_mapping {
        Some(m) => m,
        None => return Json(json!({ "answer": "抱歉，我没有识别出您提到的业务维度。" })).into_response(),
    };

    // 2. 组装 SQL (MVP 演示：单表条件查询)
    // 安全提醒：实际开发务必防范 SQL 注入，此处使用占位符
    let sql = format!(
        "SELECT * FROM {} WHERE {} = $1 LIMIT 10",
        mapping.target_table, mapping.target_column
    );

    // 3. 执行查询
    // 这里我们直接复用 state.db (即外部数据源和 SSE 元数据在同一个库，或者你可以再接一个 Pool)
    let rows_result = sqlx::query(&sql)
        .bind(filter_value)
        .fetch_all(&state.db)
        .await;

    match rows_result {
        Ok(rows) => {
            let data: Vec<serde_json::Value> = rows.iter().map(pg_row_to_json).collect();
            
            Json(json!({
                "status": "success",
                "matched_entity": mapping.entity_label,
                "data": data,
                "meta": {
                    "sql": sql,
                    "row_count": data.len()
                }
            })).into_response()
        },
        Err(e) => Json(json!({ "error": format!("执行查询失败: {}", e) })).into_response(),
    }
}