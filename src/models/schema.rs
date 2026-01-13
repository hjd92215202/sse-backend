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
    pub source_id: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateMappingRequest {
    pub entity_key: String,
    pub entity_label: String,
    pub alias_names: Vec<String>,
    pub target_table: String,
    pub target_column: String,
    pub source_id: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct DataSource {
    pub id: String,
    pub db_type: String,
    pub connection_url: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDataSourceRequest {
    pub id: String,
    pub db_type: String, // "postgres" 或 "mysql"
    pub connection_url: String,
    pub display_name: Option<String>,
}