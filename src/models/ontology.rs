use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum Operator {
    Eq, Gt, Lt, Gte, Lte, Like, Sum, Avg, Count
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QueryConstraint {
    pub column: String,
    pub operator: Operator,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct InferencePath {
    pub target_tables: Vec<String>,
    pub join_conditions: Vec<String>,
    pub filters: Vec<String>,
    pub aggregation: Option<Operator>,
}