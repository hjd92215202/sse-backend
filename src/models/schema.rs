use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BusinessConstraint {
    pub column: String,
    pub operator: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize, FromRow, Clone)]
pub struct FullSemanticNode {
    pub id: Uuid,
    pub node_key: String,
    pub label: String,
    pub node_type: String,
    pub source_id: String,
    pub target_table: String,
    pub target_column: String,
    pub default_constraints: sqlx::types::Json<Vec<BusinessConstraint>>,
    pub alias_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateNodeRequest {
    pub node_key: String,
    pub label: String,
    pub node_type: String,
    pub source_id: String,
    pub target_table: String,
    pub target_column: String,
    pub alias_names: Vec<String>,
    pub default_constraints: Vec<BusinessConstraint>,
}

#[derive(Debug, Serialize, Deserialize, FromRow, Clone)]
pub struct DataSource {
    pub id: String,
    pub db_type: String,
    pub connection_url: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDataSourceRequest {
    pub id: String,
    pub db_type: String,
    pub connection_url: String,
    pub display_name: Option<String>,
}