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

    // 使用 instrument 宏自动创建一个名为 infer 的日志范围，跳过 state 的记录以免日志冗余
    #[instrument(skip(self, state), fields(query = %query))]
    pub async fn infer(
        &self,
        state: Arc<AppState>,
        query: &str,
    ) -> anyhow::Result<InferenceResult> {
        let fst = state.fst.read().await;
        info!("开始进行语义推理: {}", query);

        let words = self.jieba.cut(query, false);
        debug!("分词结果: {:?}", words);

        let mut target_metrics = Vec::new();
        let mut potential_dims = Vec::new();

        // A. 预提取日期
        let date_regex = Regex::new(r"(\d{4}-\d{2}-\d{2})").unwrap();
        let captured_date = date_regex.captures(query).map(|cap| cap[1].to_string());
        if let Some(ref d) = captured_date {
            info!("检测到日期文本: {}", d);
        }

        // B. 语义片段匹配
        for word in words {
            let w = word.to_lowercase();

            // 1. 匹配 FST 词库 (Label 或别名)
            for entry in fst.node_cache.iter() {
                let n = entry.value();
                if n.label == w || n.alias_names.contains(&w) {
                    debug!("FST 命中节点: {} (Role: {})", n.label, n.node_role);
                    if n.node_role == "METRIC" {
                        target_metrics.push(n.clone());
                    }
                    // 如果是维度且包含时间语义，且捕获到了日期，直接锁定
                    if n.node_role == "DIMENSION"
                        && captured_date.is_some()
                        && (n.node_key.contains("date") || n.label.contains("时间"))
                    {
                        debug!(
                            "自动绑定日期 {} -> 维度 {}",
                            captured_date.as_ref().unwrap(),
                            n.label
                        );
                        potential_dims.push((n.clone(), captured_date.clone().unwrap()));
                    }
                }
            }

            // 2. 匹配 A-Box (码值库推理)
            let val_rows = sqlx::query(
                "SELECT dimension_node_id, value_code, value_label FROM dimension_values WHERE value_label = $1",
            )
            .bind(word)
            .fetch_all(&state.db)
            .await?;

            for row in val_rows {
                let dim_id: Uuid = row.get("dimension_node_id");
                let code: String = row.get("value_code");
                let val_label: String = row.get("value_label");
                // 找到对应的维度定义
                if let Some(dim_node) = fst.node_cache.iter().find(|e| e.value().id == dim_id) {
                    debug!(
                        "A-Box 识别码值实例: {} -> 归属维度: {}",
                        val_label,
                        dim_node.value().label
                    );
                    potential_dims.push((dim_node.value().clone(), code));
                }
            }
        }

        // C. 意图锚点判定
        if target_metrics.is_empty() {
            warn!("推理失败: 无法在提问中找到指标锚点");
            return Err(anyhow::anyhow!("未能在本体中定位到指标锚点"));
        }

        // 如果有多个指标，暂取第一个（未来可增加指标消歧逻辑）
        let metric = target_metrics[0].clone();
        info!("锁定指标锚点: {} (ID: {})", metric.label, metric.id);

        // D. 语义消歧 (核心推理：解决同名维度冲突)
        // 仅保留指标 T-Box 显式关联的维度
        debug!("开始执行 T-Box 关联消歧...");
        let supported_dim_ids: HashSet<Uuid> = sqlx::query!(
            "SELECT dimension_node_id FROM metric_dimension_rels WHERE metric_node_id = $1",
            metric.id
        )
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|r| r.dimension_node_id)
        .collect();

        debug!("该指标允许的维度 ID 集合: {:?}", supported_dim_ids);

        let mut final_filters = Vec::new();
        for (dim, val) in potential_dims {
            if supported_dim_ids.contains(&dim.id) {
                info!(
                    "验证通过: 维度 '{}' (值: {}) 与指标 '{}' 连通",
                    dim.label, val, metric.label
                );
                final_filters.push((dim, val));
            } else {
                warn!(
                    "丢弃冲突维度: '{}' 不在指标 '{}' 的 T-Box 关联范围内",
                    dim.label, metric.label
                );
            }
        }

        Ok(InferenceResult {
            metric,
            filters: final_filters,
        })
    }
}
