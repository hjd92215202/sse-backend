use sqlx::{Pool, Postgres, MySql, postgres::PgPoolOptions, mysql::MySqlPoolOptions, Row};
use dashmap::DashMap;
use std::sync::Arc;
use crate::models::schema::DataSource;

pub enum DynamicPool {
    Postgres(Pool<Postgres>),
    MySql(Pool<MySql>),
}

pub struct PoolManager {
    pools: DashMap<String, Arc<DynamicPool>>,
}

impl PoolManager {
    pub fn new() -> Self {
        Self {
            pools: DashMap::new(),
        }
    }

    /// 探测外部数据库的所有表名
        pub async fn list_tables(&self, source: &DataSource) -> anyhow::Result<Vec<String>> {
        let pool = self.get_or_create_pool(source).await?;
        match &*pool {
            DynamicPool::Postgres(p) => {
                // 使用非宏方式避免 Option 问题，或使用 filter_map
                let rows = sqlx::query("SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public'")
                    .fetch_all(p).await?;
                Ok(rows.into_iter().filter_map(|r| r.try_get::<String, _>(0).ok()).collect())
            },
            DynamicPool::MySql(p) => {
                let rows = sqlx::query("SHOW TABLES").fetch_all(p).await?;
                Ok(rows.into_iter().filter_map(|r| r.try_get::<String, _>(0).ok()).collect())
            }
        }
    }

    /// 探测指定表的所有列名
        pub async fn list_columns(&self, source: &DataSource, table: &str) -> anyhow::Result<Vec<String>> {
        let pool = self.get_or_create_pool(source).await?;
        match &*pool {
            DynamicPool::Postgres(p) => {
                let rows = sqlx::query("SELECT column_name FROM information_schema.columns WHERE table_name = $1")
                    .bind(table)
                    .fetch_all(p).await?;
                Ok(rows.into_iter().filter_map(|r| r.try_get::<String, _>(0).ok()).collect())
            },
            DynamicPool::MySql(p) => {
                let rows = sqlx::query(&format!("DESCRIBE {}", table)).fetch_all(p).await?;
                Ok(rows.into_iter().filter_map(|r| r.try_get::<String, _>(0).ok()).collect())
            }
        }
    }

    pub async fn get_or_create_pool(&self, source: &DataSource) -> anyhow::Result<Arc<DynamicPool>> {
        if let Some(pool) = self.pools.get(&source.id) {
            return Ok(pool.clone());
        }
        let new_pool = match source.db_type.to_lowercase().as_str() {
            "postgres" | "postgresql" => {
                let pool = PgPoolOptions::new().max_connections(5).connect(&source.connection_url).await?;
                Arc::new(DynamicPool::Postgres(pool))
            }
            "mysql" => {
                let pool = MySqlPoolOptions::new().max_connections(5).connect(&source.connection_url).await?;
                Arc::new(DynamicPool::MySql(pool))
            }
            _ => return Err(anyhow::anyhow!("Unsupported DB type")),
        };
        self.pools.insert(source.id.clone(), new_pool.clone());
        Ok(new_pool)
    }
}