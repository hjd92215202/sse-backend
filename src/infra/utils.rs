use sqlx::{Column, Row, TypeInfo, postgres::PgRow};
use serde_json::{json, Value, Map};
use rust_decimal::prelude::ToPrimitive;
use chrono::{NaiveDate, NaiveDateTime};

pub fn pg_row_to_json(row: &PgRow) -> Value {
    let mut map = Map::new();
    
    for col in row.columns() {
        let name = col.name();
        let type_info = col.type_info();
        let type_name = type_info.name(); // 获取数据库原生类型名，如 "INT4", "NUMERIC"

        let val = match type_name {
            "INT2" | "INT4" => {
                let v: Option<i32> = row.try_get(name).unwrap_or(None);
                json!(v)
            }
            "INT8" => {
                let v: Option<i64> = row.try_get(name).unwrap_or(None);
                json!(v)
            }
            "FLOAT4" | "FLOAT8" => {
                let v: Option<f64> = row.try_get(name).unwrap_or(None);
                json!(v)
            }
            "NUMERIC" => {
                let v: Option<rust_decimal::Decimal> = row.try_get(name).unwrap_or(None);
                // 金额建议转为字符串保持精度，或者转为 f64
                json!(v.map(|d| d.to_f64().unwrap_or(0.0)))
            }
            "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" => {
                let v: Option<String> = row.try_get(name).unwrap_or(None);
                json!(v)
            }
            "BOOL" => {
                let v: Option<bool> = row.try_get(name).unwrap_or(None);
                json!(v)
            }
            "DATE" => {
                let v: Option<NaiveDate> = row.try_get(name).unwrap_or(None);
                json!(v.map(|d| d.to_string()))
            }
            "TIMESTAMP" | "TIMESTAMPTZ" => {
                let v: Option<NaiveDateTime> = row.try_get(name).unwrap_or(None);
                json!(v.map(|dt| dt.to_string()))
            }
            "JSON" | "JSONB" => {
                let v: Option<Value> = row.try_get(name).unwrap_or(None);
                v.unwrap_or(Value::Null)
            }
            _ => {
                // 对于未知类型，尝试转为字符串
                let v: Option<String> = row.try_get(name).unwrap_or(None);
                json!(v)
            }
        };
        
        map.insert(name.to_string(), val);
    }
    
    Value::Object(map)
}