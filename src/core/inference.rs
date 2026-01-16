use crate::ax_state::AppState;
use crate::models::schema::FullSemanticNode;
use jieba_rs::Jieba;
use regex::Regex;
use sqlx::Row;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

pub struct SemanticInferenceEngine {
    jieba: Jieba,
}

#[derive(Debug)]
pub struct InferenceResult {
    pub metric: FullSemanticNode,
    pub filters: Vec<(FullSemanticNode, String)>, // (维度节点, 物理值)
}

impl SemanticInferenceEngine {
    pub fn new() -> Self {
        Self {
            jieba: Jieba::new(),
        }
    }

    /// 【核心功能】热更新分词词典
    /// 将本体标签、别名、维度码值全部注入分词器，权重设为最高 (100)
    pub fn refresh_custom_words(&mut self, words: Vec<String>) {
        let cnt = words.len();
        for word in words {
            // 注入词典：词语、权重、词性
            self.jieba.add_word(&word, Some(100), Some("n"));
        }
        info!("分词器自定义词典已热重载，新增/覆盖词汇数量: {}", cnt); // 实际数量在循环外记录更准
    }

        #[instrument(skip(self, state), fields(query = %query))]
    pub async fn infer(&self, state: Arc<AppState>, query: &str) -> anyhow::Result<InferenceResult> {
        let fst = state.fst.read().await;
        let words = self.jieba.cut(query, false);
        debug!("分词结果: {:?}", words);

        let mut target_metrics = Vec::new();
        let mut potential_dims = Vec::new(); // 维度实例匹配
        let mut hit_dimension_types = HashSet::new(); // 已命中的维度类型节点

        // A. 预提取：通用正则捕获
        let date_regex = Regex::new(r"(\d{4}-\d{2}-\d{2})").unwrap();
        let captured_date = date_regex.captures(query).map(|cap| cap[1].to_string());

        // B. 词法/语义扫描
        for (idx, word) in words.iter().enumerate() {
            let w = word.to_lowercase();

            // 1. 尝试匹配 FST (本体类识别)
            for entry in fst.node_cache.iter() {
                let n = entry.value();
                if n.label == w || n.alias_names.contains(&w) {
                    if n.node_role == "METRIC" {
                        target_metrics.push(n.clone());
                    } else if n.node_role == "DIMENSION" {
                        hit_dimension_types.insert(n.id);
                        
                        // 【优化】：动态值推断
                        // 如果用户说“结算平台是C公司”，FST 命中了“结算平台”，
                        // 逻辑：看后面一个词（words[idx+1]），如果不是指标且不是已知码值，视为动态 Value
                        if idx + 1 < words.len() {
                            let next_word = words[idx + 1].trim();
                            if next_word != "是" && next_word != "为" && next_word.len() > 1 {
                                // 简单判定：如果 next_word 在 FST 没命中，就当它是值
                                potential_dims.push((n.clone(), next_word.to_string()));
                            }
                        }
                    }
                }
            }

            // 2. 尝试匹配 A-Box (码值索引)
            let val_rows = sqlx::query("SELECT dimension_node_id, value_code FROM dimension_values WHERE value_label = $1")
                .bind(*word).fetch_all(&state.db).await?;
            for row in val_rows {
                let dim_id: Uuid = row.get(0);
                if let Some(dn) = fst.node_cache.iter().find(|e| e.value().id == dim_id) {
                    potential_dims.push((dn.value().clone(), row.get(1)));
                }
            }
        }

        // C. 锚点确定
        if target_metrics.is_empty() {
            warn!("未识别到指标锚点");
            return Err(anyhow::anyhow!("未识别到指标锚点"));
        }
        let metric = target_metrics[0].clone();

        // D. 本体关系推理 (T-Box Check & Auto-Binding)
        let supported_dim_ids: HashSet<Uuid> = sqlx::query!("SELECT dimension_node_id FROM metric_dimension_rels WHERE metric_node_id = $1", metric.id)
            .fetch_all(&state.db).await?.into_iter().map(|r| r.dimension_node_id).collect();

        let mut final_filters = Vec::new();

        // 1. 处理 A-Box 或 动态推断出的过滤值
        for (dim, val) in potential_dims {
            if supported_dim_ids.contains(&dim.id) {
                final_filters.push((dim, val));
            }
        }

        // 2. 【核心优化】：处理时间类推断
        // 如果捕获到了日期，但维度实例匹配没结果，则寻找该指标关联的所有 DATE 类型的维度
        if captured_date.is_some() {
            let date_val = captured_date.unwrap();
            for dim_id in &supported_dim_ids {
                if let Some(dim_node) = fst.node_cache.iter().find(|e| e.value().id == *dim_id) {
                    if dim_node.value().semantic_type == "DATE" {
                        // 自动绑定日期到该指标关联的时间列
                        info!("基于 T-Box 类型推理：将日期 {} 绑定至维度 {}", date_val, dim_node.value().label);
                        final_filters.push((dim_node.value().clone(), date_val.clone()));
                    }
                }
            }
        }

        Ok(InferenceResult { metric, filters: final_filters })
    }
}
