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
    pub node_role: String, // METRIC / DIMENSION
    pub source_id: String,
    pub target_table: String,
    pub sql_expression: String,
    // 使用 sqlx::types::Json 包装自定义结构体数组
    pub default_constraints: sqlx::types::Json<Vec<BusinessConstraint>>,
    // Postgres 的 TEXT[] 对应 Rust 的 Vec<String>
    pub alias_names: Vec<String>,
    pub default_agg: String,
    // 聚合查询出的维度 ID 列表
    #[sqlx(default)]
    pub supported_dimension_ids: Vec<Uuid>,
    pub dataset_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct CreateNodeRequest {
    pub node_key: String,
    pub label: String,
    pub node_role: String,
    pub source_id: String,
    pub target_table: String,
    pub sql_expression: String,
    pub alias_names: Vec<String>,
    pub default_constraints: Vec<BusinessConstraint>,
    pub supported_dimension_ids: Vec<Uuid>,
    pub default_agg: String,
    pub dataset_id: Option<Uuid>,
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

#[derive(Debug, Deserialize)]
pub struct MetadataRequest {
    pub source_id: String,
    pub table_name: Option<String>,
}

/// 吸收自 SuperSonic 的逻辑查询计划中间表达
/// 目前在推理机中直接生成 SQL，但在多表关联阶段将由该结构体承载推理状态
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QueryLogicalPlan {
    pub metric: FullSemanticNode,
    pub dimensions: Vec<(FullSemanticNode, String)>,
    pub implicit_filters: Vec<String>,
    pub final_agg: String,
    pub dataset_context: Option<Uuid>,
}