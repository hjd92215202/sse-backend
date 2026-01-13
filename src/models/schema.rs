use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, FromRow, Clone)]
pub struct SemanticMapping {
    pub id: Uuid,
    pub entity_key: String,
    pub entity_label: String,
    pub alias_names: Vec<String>,
    pub target_table: String,
    pub target_column: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateMappingRequest {
    pub entity_key: String,
    pub entity_label: String,
    pub alias_names: Vec<String>,
    pub target_table: String,
    pub target_column: String,
}