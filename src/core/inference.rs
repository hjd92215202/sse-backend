use crate::models::ontology::{Operator, QueryConstraint, InferencePath};
use crate::models::schema::SemanticMapping;

pub struct SemanticReasoner {
    // 这里未来可以加载全量本体图
}

impl SemanticReasoner {
    /// 核心逻辑：将识别出的实体和算子，转化为逻辑执行路径
    pub fn reason(
        &self, 
        matched_entities: Vec<SemanticMapping>, 
        raw_query: &str
    ) -> InferencePath {
        let mut path = InferencePath {
            target_tables: Vec::new(),
            join_conditions: Vec::new(),
            filters: Vec::new(),
            aggregation: None,
        };

        // 1. 逻辑算子识别 (简单的规则引擎)
        if raw_query.contains("总") || raw_query.contains("合计") {
            path.aggregation = Some(Operator::Sum);
        } else if raw_query.contains("平均") {
            path.aggregation = Some(Operator::Avg);
        }

        // 2. 数值约束识别 (正则解析 > 100 等)
        // 此处简化处理：将 matched_entities 中的物理映射提取出来
        for m in matched_entities {
            if !path.target_tables.contains(&m.target_table) {
                path.target_tables.push(m.target_table.clone());
            }
            // 记录隐含约束（这里可以从数据库读）
            // path.filters.push("status = 'ACTIVE'".to_string());
        }

        // 3. 自动补全 JOIN 路径 (推理核心)
        if path.target_tables.len() > 1 {
            // 如果涉及多表，根据 ontology_edges 推理 JOIN 条件
            // 演示：硬编码推理逻辑
            if path.target_tables.contains(&"t_platform_dim".to_string()) && 
               path.target_tables.contains(&"t_revenue_data".to_string()) {
                path.join_conditions.push("t_revenue_data.platform_id = t_platform_dim.id".to_string());
            }
        }

        path
    }
}