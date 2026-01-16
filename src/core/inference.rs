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

    /// 热更新分词词典
    pub fn refresh_custom_words(&mut self, words: Vec<String>) {
        let cnt = words.len();
        for word in words {
            self.jieba.add_word(&word, Some(100), Some("n"));
        }
        info!("分词器自定义词典已热重载，新增词汇数量: {}", cnt);
    }

    #[instrument(skip(self, state), fields(query = %query))]
    pub async fn infer(
        &self,
        state: Arc<AppState>,
        query: &str,
    ) -> anyhow::Result<InferenceResult> {
        let fst = state.fst.read().await;
        info!("🧠 启动语义推理流水线...");

        // 1. 预解析：正则捕获日期 (YYYY-MM-DD)
        let date_regex = Regex::new(r"(\d{4}-\d{2}-\d{2})").unwrap();
        let captured_date = date_regex.captures(query).map(|cap| cap[1].to_string());
        if let Some(ref d) = captured_date {
            info!("📍 识别到日期特征串: {}", d);
        }

        // 2. 语义分词
        let words = self.jieba.cut(query, false);
        debug!("分词 Token 序列: {:?}", words);

        let mut target_metrics = Vec::new();
        // 候选池：记录所有识别到的 (维度节点, 提取到的值)
        let mut raw_candidates = Vec::new();

        // 3. 扫描识别
        for (idx, word) in words.iter().enumerate() {
            let w = word.to_lowercase();

            // A. FST 匹配 (识别指标名和维度名)
            for entry in fst.node_cache.iter() {
                let n = entry.value();
                if n.label == w || n.alias_names.contains(&w) {
                    if n.node_role == "METRIC" {
                        target_metrics.push(n.clone());
                    } else if n.node_role == "DIMENSION" {
                        debug!("FST 命中维度定义: {}", n.label);
                        // 动态值推断逻辑：如果后面跟着一个非指标且非“是/为”的词，捕获为动态 Value
                        if idx + 1 < words.len() {
                            let next_word = words[idx + 1].trim();
                            if next_word.len() > 1 && next_word != "是" && next_word != "为" {
                                debug!("基于上下文捕获动态值: {} -> {}", n.label, next_word);
                                raw_candidates.push((n.clone(), next_word.to_string()));
                            }
                        }
                    }
                }
            }

            // B. A-Box 匹配 (在码值实例库中精准搜索)
            let val_rows = sqlx::query(
                "SELECT dimension_node_id, value_code FROM dimension_values WHERE value_label = $1",
            )
            .bind(*word)
            .fetch_all(&state.db)
            .await?;

            for row in val_rows {
                let dim_id: Uuid = row.get(0);
                let code: String = row.get(1);
                if let Some(dn) = fst.node_cache.iter().find(|e| e.value().id == dim_id) {
                    debug!("A-Box 命中实例码值: {} -> {}", dn.value().label, word);
                    raw_candidates.push((dn.value().clone(), code));
                }
            }
        }

        // 4. 意图锚点确定
        if target_metrics.is_empty() {
            warn!("推理失败：未能在提问中定位到任何业务指标");
            return Err(anyhow::anyhow!("未识别到指标锚点，请明确提问目标（如：收益、应还）"));
        }
        let metric = target_metrics[0].clone();
        info!("🎯 锁定指标锚点: {}", metric.label);

        // 5. T-Box 语义合规性验证与去重
        // 获取当前指标在本体中关联的所有有效维度 ID
        let supported_dim_ids: HashSet<Uuid> = sqlx::query!(
            "SELECT dimension_node_id FROM metric_dimension_rels WHERE metric_node_id = $1",
            metric.id
        )
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|r| r.dimension_node_id)
        .collect();

        let mut final_filters = Vec::new();
        let mut seen_pairs = HashSet::new();

        // A. 校验并合并来自 A-Box 和上下文捕获的过滤器
        for (dim, val) in raw_candidates {
            if supported_dim_ids.contains(&dim.id) {
                let pair_key = (dim.id, val.clone());
                if !seen_pairs.contains(&pair_key) {
                    info!("✅ 语义绑定成功: {} = '{}'", dim.label, val);
                    seen_pairs.insert(pair_key);
                    final_filters.push((dim, val));
                }
            }
        }

        // B. 自动处理时间维度绑定 (基于类型推理)
        // 如果捕获到了日期，寻找该指标关联的 DATE 类型维度，且该维度目前还没被绑定值
        if let Some(date_val) = captured_date {
            for dim_id in &supported_dim_ids {
                if let Some(dim_node) = fst.node_cache.iter().find(|e| e.value().id == *dim_id) {
                    let n = dim_node.value();
                    // 如果该维度是日期类型，且本次推理中还没给它分配过值
                    if n.semantic_type == "DATE" && !seen_pairs.iter().any(|(id, _)| id == &n.id) {
                        info!("📅 基于 T-Box 类型推理：自动将日期 '{}' 绑定至时间维度 '{}'", date_val, n.label);
                        final_filters.push((n.clone(), date_val.clone()));
                        seen_pairs.insert((n.id, date_val.clone()));
                    }
                }
            }
        }

        Ok(InferenceResult {
            metric,
            filters: final_filters,
        })
    }
}