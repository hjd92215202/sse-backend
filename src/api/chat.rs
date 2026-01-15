use ax_state::AppState;
use axum::{extract::State, response::IntoResponse, Json};
use regex::Regex;
use serde_json::json;
use sqlx::Row;
use std::sync::Arc;
use std::collections::HashSet;

use crate::ax_state;
use crate::infra::db_external::DynamicPool;
use crate::infra::db_internal::{mysql_row_to_json, pg_row_to_json};
use crate::models::context::ChatRequest;
use crate::models::schema::{DataSource, FullSemanticNode};

pub async fn chat_query(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ChatRequest>,
) -> impl IntoResponse {
    let query_text = payload.query.trim();
    let fst = state.fst.read().await;

    // --- 1. 语义识别阶段 ---
    let mut target_metrics: Vec<FullSemanticNode> = Vec::new();
    let mut raw_filters: Vec<(FullSemanticNode, String)> = Vec::new();

    // A. 时间正则捕获
    let date_regex = Regex::new(r"(\d{4}-\d{2}-\d{2})").unwrap();
    let captured_date = date_regex.captures(query_text).map(|cap| cap[1].to_string());

    for entry in fst.node_cache.iter() {
        let n = entry.value();

        // 识别指标
        if n.node_role == "METRIC"
            && (query_text.contains(&n.label) || n.alias_names.iter().any(|a| query_text.contains(a)))
        {
            target_metrics.push(n.clone());
        }

        // 识别维度
        if n.node_role == "DIMENSION" {
            // 逻辑 1: 时间正则匹配
            let is_time = n.node_key.to_lowercase().contains("date")
                || n.label.contains("时间")
                || n.label.contains("日期");
            if is_time && captured_date.is_some() {
                raw_filters.push((n.clone(), captured_date.clone().unwrap()));
            }

            // 逻辑 2: A-Box 实例库匹配 (码值匹配)
            let instances = sqlx::query(
                "SELECT value_label, value_code FROM dimension_values WHERE dimension_node_id = $1",
            )
            .bind(n.id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

            for inst in instances {
                let label: String = inst.get("value_label");
                let code: String = inst.get("value_code");
                if query_text.contains(&label) {
                    raw_filters.push((n.clone(), code));
                }
            }
        }
    }

    // --- 核心修复：对过滤器进行去重 ---
    let mut detected_filters: Vec<(FullSemanticNode, String)> = Vec::new();
    let mut seen_keys = HashSet::new();

    for (node, val) in raw_filters {
        // 创建唯一键：维度ID + 过滤值
        let key = format!("{}:{}", node.id, val);
        if !seen_keys.contains(&key) {
            seen_keys.insert(key);
            detected_filters.push((node, val));
        }
    }

    // --- 2. 意图对齐 ---
    if target_metrics.is_empty() {
        return Json(json!({
            "status": "fail", 
            "answer": "未识别到指标，请尝试输入具体指标名称（如：收益额）。"
        })).into_response();
    }
    
    if target_metrics.len() > 1 {
        return Json(json!({
            "status": "ambiguous", 
            "answer": "发现多个相似指标，请确认您的意图", 
            "candidates": target_metrics.iter().map(|n| n.label.clone()).collect::<Vec<_>>()
        })).into_response();
    }

    let metric = &target_metrics[0];

    // --- 3. 语义连通性验证 (T-Box Check) ---
    for (dim, _) in &detected_filters {
        let is_valid = sqlx::query(
            "SELECT 1 FROM metric_dimension_rels WHERE metric_node_id = $1 AND dimension_node_id = $2",
        )
        .bind(metric.id)
        .bind(dim.id)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

        if is_valid.is_none() {
            return Json(json!({
                "status": "ambiguous",
                "answer": format!("业务语义警告：指标 '{}' 并不支持按 '{}' 维度分析。", metric.label, dim.label)
            })).into_response();
        }
    }

    // --- 4. SQL 组装 ---
    let mut select_items = Vec::new();
    let mut group_by_items = Vec::new();

    // 维度列
    for (dim_node, _) in &detected_filters {
        select_items.push(format!("{} as \"{}\"", dim_node.target_column, dim_node.label));
        group_by_items.push(dim_node.target_column.clone());
    }

    // 指标聚合
    let agg = if query_text.contains("平均") {
        "AVG"
    } else if metric.default_agg == "NONE" {
        "NONE"
    } else {
        &metric.default_agg
    };

    let metric_sql = if agg == "NONE" {
        format!("{} as \"{}\"", metric.target_column, metric.label)
    } else {
        format!("{}({}) as \"{}\"", agg, metric.target_column, metric.label)
    };
    select_items.push(metric_sql);

    // WHERE 条件
    let mut where_conds = vec!["1=1".to_string()];
    for (dim_node, val) in &detected_filters {
        where_conds.push(format!("{} = '{}'", dim_node.target_column, val));
    }

    // 注入业务约束
    for c in &metric.default_constraints.0 {
        where_conds.push(format!("{} {} '{}'", c.column, c.operator, c.value));
    }

    let select_clause = select_items.join(", ");
    let where_clause = where_conds.join(" AND ");
    let mut sql = format!("SELECT {} FROM {} WHERE {}", select_clause, metric.target_table, where_clause);

    if agg != "NONE" && !group_by_items.is_empty() {
        sql.push_str(&format!(" GROUP BY {}", group_by_items.join(", ")));
    }

    // --- 5. 执行 ---
    let source_row = sqlx::query("SELECT id, db_type, connection_url, display_name FROM data_sources WHERE id = $1")
        .bind(&metric.source_id)
        .fetch_one(&state.db)
        .await;

    let source = match source_row {
        Ok(r) => DataSource {
            id: r.get("id"),
            db_type: r.get("db_type"),
            connection_url: r.get("connection_url"),
            display_name: r.get("display_name"),
        },
        Err(_) => return Json(json!({"status": "error", "message": "无法找到映射的数据源"})).into_response(),
    };

    let pool = state.pool_manager.get_or_create_pool(&source).await.unwrap();

    match &*pool {
        DynamicPool::Postgres(p) => {
            let rows = sqlx::query(&sql).fetch_all(p).await.unwrap_or_default();
            Json(json!({
                "status": "success", 
                "sql": sql, 
                "logic": format!("基于{}聚合", agg),
                "data": rows.iter().map(pg_row_to_json).collect::<Vec<serde_json::Value>>()
            })).into_response()
        },
        DynamicPool::MySql(p) => {
            let rows = sqlx::query(&sql).fetch_all(p).await.unwrap_or_default();
            Json(json!({
                "status": "success", 
                "sql": sql, 
                "logic": format!("基于{}聚合", agg),
                "data": rows.iter().map(mysql_row_to_json).collect::<Vec<serde_json::Value>>()
            })).into_response()
        }
    }
}