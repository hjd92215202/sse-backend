use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;
use std::sync::Arc;

// 导入项目内部组件
use crate::ax_state::AppState;
use crate::models::context::ChatRequest;
use crate::models::schema::DataSource; // 保持导入
use crate::infra::db_external::DynamicPool;
use crate::infra::db_internal::{pg_row_to_json, mysql_row_to_json};
use tracing::{info, warn, error, instrument};

/// 语义问数对话核心接口
#[instrument(skip(state, payload), fields(user_query = %payload.query))]
pub async fn chat_query(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ChatRequest>,
) -> impl IntoResponse {
    let query_text = payload.query.trim();

    // 1. 获取推理引擎单例（已预装载自定义词典）
    let engine = state.engine.read().await;

    // 2. 执行深度语义推理
    let inference = match engine.infer(state.clone(), query_text).await {
        Ok(res) => res,
        Err(e) => {
            warn!("语义推理未命中: {}", e);
            return Json(json!({
                "status": "fail",
                "answer": format!("抱歉，我理解不了这个提问：{}", e)
            }))
            .into_response();
        }
    };

    let metric = inference.metric;
    let filters = inference.filters;

    // 3. 确定聚合逻辑
    let agg = if query_text.contains("平均") {
        "AVG"
    } else if metric.default_agg == "NONE" {
        "NONE"
    } else {
        &metric.default_agg
    };

    // 4. 构造 SELECT 子句
    let metric_item = if agg == "NONE" {
        format!("{} as \"{}\"", metric.sql_expression, metric.label)
    } else {
        format!("{}({}) as \"{}\"", agg, metric.sql_expression, metric.label)
    };

    // 5. 组装 SQL 片段
    let mut select_items = vec![metric_item];
    let mut where_conds = vec!["1=1".to_string()];
    let mut group_by_items = Vec::new();

    for (dim_node, val_code) in &filters {
        where_conds.push(format!("{} = '{}'", dim_node.sql_expression, val_code));
        select_items.insert(
            0,
            format!("{} as \"{}\"", dim_node.sql_expression, dim_node.label),
        );
        if agg != "NONE" {
            group_by_items.push(dim_node.sql_expression.clone());
        }
    }

    // 6. 注入业务隐含约束
    for c in &metric.default_constraints.0 {
        where_conds.push(format!("{} {} '{}'", c.column, c.operator, c.value));
    }
    for (dim_node, _) in &filters {
        for c in &dim_node.default_constraints.0 {
            where_conds.push(format!("{} {} '{}'", c.column, c.operator, c.value));
        }
    }

    // 7. 拼装物理 SQL
    let select_clause = select_items.join(", ");
    let where_clause = where_conds.join(" AND ");
    let mut sql = format!(
        "SELECT {} FROM {} WHERE {}",
        select_clause, metric.target_table, where_clause
    );

    if !group_by_items.is_empty() {
        sql.push_str(&format!(" GROUP BY {}", group_by_items.join(", ")));
    }

    info!("🚀 语义推理完成，生成 SQL: {}", sql);

    // 8. 动态路由数据源 (修复点：直接使用导入的 DataSource 类型)
    let source_res: Result<DataSource, _> =
        sqlx::query_as("SELECT * FROM data_sources WHERE id = $1")
            .bind(&metric.source_id)
            .fetch_one(&state.db)
            .await;

    let source = match source_res {
        Ok(s) => s,
        Err(_) => {
            error!("无法找到该指标对应的数据源配置");
            return Json(json!({"status": "error", "message": "无法找到该指标对应的数据源配置"}))
                .into_response()
        }
    };

    let pool = match state.pool_manager.get_or_create_pool(&source).await {
        Ok(p) => p,
        Err(e) => {
            error!("无法建立数据库连接");
            return Json(json!({"status": "error", "message": format!("无法建立数据库连接: {}", e)}))
                .into_response()
        }
    };

    let start_time = std::time::Instant::now();

    // 9. 执行查询
    match &*pool {
        DynamicPool::Postgres(p) => {
            let rows_result = sqlx::query(&sql).fetch_all(p).await;
            match rows_result {
                Ok(rows) => {
                    let data: Vec<serde_json::Value> = rows.iter().map(pg_row_to_json).collect();
                    info!(
                        "✅ 查询成功 - 耗时: {:?}, 返回 {} 行",
                        start_time.elapsed(),
                        rows.len()
                    );
                    Json(json!({
                        "status": "success",
                        "sql": sql,
                        "logic": format!("指标: {}, 关联维度: {}, 聚合: {}", metric.label, filters.len(), agg),
                        "data": data
                    })).into_response()
                }
                Err(e) => {
                    error!("SQL执行失败: {}", e);
                    Json(json!({"status": "error", "message": format!("物理库执行失败: {}", e)}))
                        .into_response()
                }
            }
        }
        DynamicPool::MySql(p) => {
            let sql_mysql = sql.replace("$1", "?"); 
            let rows_result = sqlx::query(&sql_mysql).fetch_all(p).await;
            match rows_result {
                Ok(rows) => {
                    let data: Vec<serde_json::Value> = rows.iter().map(mysql_row_to_json).collect();
                    Json(json!({
                        "status": "success",
                        "sql": sql_mysql,
                        "logic": format!("指标: {}, 关联维度: {}, 聚合: {}", metric.label, filters.len(), agg),
                        "data": data
                    })).into_response()
                }
                Err(e) => {
                    Json(json!({"status": "error", "message": format!("MySQL执行失败: {}", e)}))
                        .into_response()
                }
            }
        }
    }
}