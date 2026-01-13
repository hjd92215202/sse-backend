use fst::{Map, MapBuilder};
use std::collections::BTreeMap;
use dashmap::DashMap;
use std::sync::Arc;
use crate::models::schema::SemanticMapping;

pub struct FstEngine {
    index: Map<Vec<u8>>,
    // 存储 ID 到映射详情的快速反查
    pub mapping_cache: Arc<DashMap<u64, SemanticMapping>>,
}

impl FstEngine {
    pub fn build(mappings: &[SemanticMapping]) -> anyhow::Result<Self> {
        let mut builder = MapBuilder::memory();
        let cache = Arc::new(DashMap::new());
        
        // FST 键必须有序
        let mut data: BTreeMap<String, u64> = BTreeMap::new();
        
        for (idx, m) in mappings.iter().enumerate() {
            let id = idx as u64;
            data.insert(m.entity_label.to_lowercase(), id);
            for alias in &m.alias_names {
                data.insert(alias.to_lowercase(), id);
            }
            cache.insert(id, m.clone());
        }

        for (key, id) in data {
            builder.insert(key, id)?;
        }

        let bytes = builder.into_inner()?;
        Ok(Self {
            index: Map::new(bytes)?,
            mapping_cache: cache,
        })
    }

    // 简单匹配：输入文本，返回对应的映射详情
    pub fn find_match(&self, query: &str) -> Option<SemanticMapping> {
        let id = self.index.get(query.to_lowercase())?;
        self.mapping_cache.get(&id).map(|m| m.value().clone())
    }
}