use sqlx::{postgres::PgRow, Column, Row, TypeInfo, postgres::PgPoolOptions, PgPool};
use serde_json::{json, Value, Map};
use sqlx::mysql::MySqlRow;
use std::env;

pub async fn init_db() -> PgPool {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to create pool")
}

// 核心转换函数：务必确保它是 pub 的
pub fn pg_row_to_json(row: &PgRow) -> Value {
    let mut map = Map::new();
    for col in row.columns() {
        let name = col.name();
        let type_info = col.type_info();
        let type_name = type_info.name();

        let val = match type_name {
            "INT2" | "INT4" => json!(row.try_get::<Option<i32>, _>(name).unwrap_or(None)),
            "INT8" => json!(row.try_get::<Option<i64>, _>(name).unwrap_or(None)),
            "FLOAT4" | "FLOAT8" => json!(row.try_get::<Option<f64>, _>(name).unwrap_or(None)),
            "NUMERIC" => {
                let v: Option<rust_decimal::Decimal> = row.try_get(name).unwrap_or(None);
                json!(v.map(|d| d.to_string())) // 或者转为 f64
            }
            "TEXT" | "VARCHAR" | "BPCHAR" => json!(row.try_get::<Option<String>, _>(name).unwrap_or(None)),
            "BOOL" => json!(row.try_get::<Option<bool>, _>(name).unwrap_or(None)),
            "DATE" => json!(row.try_get::<Option<chrono::NaiveDate>, _>(name).unwrap_or(None).map(|d| d.to_string())),
            _ => json!(row.try_get::<Option<String>, _>(name).unwrap_or(None)),
        };
        map.insert(name.to_string(), val);
    }
    Value::Object(map)
}

pub fn mysql_row_to_json(row: &MySqlRow) -> Value {
    let mut map = Map::new();
    
    for col in row.columns() {
        let name = col.name();
        let type_info = col.type_info();
        let type_name = type_info.name(); // 如 "INT", "DECIMAL", "VARCHAR", "DATE"

        let val = match type_name {
            "TINYINT" | "SMALLINT" | "INT" | "MEDIUMINT" => {
                let v: Option<i32> = row.try_get(name).unwrap_or(None);
                json!(v)
            }
            "BIGINT" => {
                let v: Option<i64> = row.try_get(name).unwrap_or(None);
                json!(v)
            }
            "FLOAT" | "DOUBLE" => {
                let v: Option<f64> = row.try_get(name).unwrap_or(None);
                json!(v)
            }
            "DECIMAL" | "NEWDECIMAL" => {
                let v: Option<rust_decimal::Decimal> = row.try_get(name).unwrap_or(None);
                json!(v.map(|d| d.to_string()))
            }
            "CHAR" | "VARCHAR" | "TEXT" | "LONGTEXT" => {
                let v: Option<String> = row.try_get(name).unwrap_or(None);
                json!(v)
            }
            "DATE" => {
                let v: Option<chrono::NaiveDate> = row.try_get(name).unwrap_or(None);
                json!(v.map(|d| d.to_string()))
            }
            "DATETIME" | "TIMESTAMP" => {
                let v: Option<chrono::NaiveDateTime> = row.try_get(name).unwrap_or(None);
                json!(v.map(|dt| dt.to_string()))
            }
            _ => {
                let v: Option<String> = row.try_get(name).unwrap_or(None);
                json!(v)
            }
        };
        
        map.insert(name.to_string(), val);
    }
    
    Value::Object(map)
}