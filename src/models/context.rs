use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub query: String, // 用户提问内容
}