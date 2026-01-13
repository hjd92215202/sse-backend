use sqlx::{Pool, Postgres, MySql, postgres::PgPoolOptions, mysql::MySqlPoolOptions};
use dashmap::DashMap;
use std::sync::Arc;
use crate::models::schema::DataSource;

// 定义动态连接池枚举
pub enum DynamicPool {
    Postgres(Pool<Postgres>),
    MySql(Pool<MySql>),
}

pub struct PoolManager {
    // 缓存已创建的连接池，避免重复建立连接
    pools: DashMap<String, Arc<DynamicPool>>,
}

impl PoolManager {
    pub fn new() -> Self {
        Self {
            pools: DashMap::new(),
        }
    }

    /// 获取或创建一个连接池
    pub async fn get_or_create_pool(&self, source: &DataSource) -> anyhow::Result<Arc<DynamicPool>> {
        // 1. 检查缓存
        if let Some(pool) = self.pools.get(&source.id) {
            return Ok(pool.clone());
        }

        // 2. 缓存未命中，创建新池
        let new_pool = match source.db_type.to_lowercase().as_str() {
            "postgres" | "postgresql" => {
                let pool = PgPoolOptions::new()
                    .max_connections(5)
                    .connect(&source.connection_url)
                    .await?;
                Arc::new(DynamicPool::Postgres(pool))
            }
            "mysql" => {
                let pool = MySqlPoolOptions::new()
                    .max_connections(5)
                    .connect(&source.connection_url)
                    .await?;
                Arc::new(DynamicPool::MySql(pool))
            }
            _ => return Err(anyhow::anyhow!("不支持的数据库类型: {}", source.db_type)),
        };

        // 3. 存入缓存并返回
        self.pools.insert(source.id.clone(), new_pool.clone());
        Ok(new_pool)
    }
}