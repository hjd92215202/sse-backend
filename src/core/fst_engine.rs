use fst::{Map, MapBuilder};
use std::collections::BTreeMap;
use dashmap::DashMap;
use std::sync::Arc;
use crate::models::schema::FullSemanticNode;

pub struct FstEngine {
    _index: Map<Vec<u8>>,
    // 缓存完整的本体知识节点
    pub node_cache: Arc<DashMap<u64, FullSemanticNode>>,
}

impl FstEngine {
    pub fn build(nodes: &[FullSemanticNode]) -> anyhow::Result<Self> {
        let mut builder = MapBuilder::memory();
        let cache = Arc::new(DashMap::new());
        let mut data: BTreeMap<String, u64> = BTreeMap::new();
        
        for (idx, n) in nodes.iter().enumerate() {
            let id = idx as u64;
            // 索引标签
            data.insert(n.label.to_lowercase(), id);
            // 索引别名
            for alias in &n.alias_names {
                data.insert(alias.to_lowercase(), id);
            }
            cache.insert(id, n.clone());
        }

        for (key, id) in data {
            builder.insert(key, id)?;
        }

        let bytes = builder.into_inner()?;
        Ok(Self {
            _index: Map::new(bytes)?,
            node_cache: cache,
        })
    }
}