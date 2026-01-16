use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;
use std::sync::Arc;

// 导入项目内部组件
use crate::ax_state::AppState;
use crate::infra::db_external::DynamicPool;
use crate::infra::db_internal::{mysql_row_to_json, pg_row_to_json};
use crate::models::context::ChatRequest;
use crate::models::schema::DataSource;
use tracing::{error, info, instrument, warn};

/// 语义问数对话核心接口
#[instrument(skip(state, payload), fields(user_query = %payload.query))]
pub async fn chat_query(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ChatRequest>,
) -> impl IntoResponse {
    let query_text = payload.query.trim();

    // 1. 初始化语义推理引擎 (内部封装了分词、FST 匹配、A-Box 识别和 T-Box 冲突消歧)
    let engine = state.engine.read().await;

    // 2. 执行语义推理
    // 该过程会自动解决：同名维度在不同表的问题、日期捕获逻辑、码值反推维度类逻辑
    let inference = match engine.infer(state.clone(), query_text).await {
        Ok(res) => res,
        Err(e) => {
            warn!("语义识别不通过: {}", e);
            return Json(json!({
                "status": "fail",
                "answer": format!("推理失败：{}", e)
            }))
            .into_response();
        }
    };

    let metric = inference.metric;
    let filters = inference.filters;

    // 3. 确定聚合逻辑
    // 策略：如果用户提问包含“平均”则强制 AVG，否则使用本体定义的默认聚合方式
    let agg = if query_text.contains("平均") {
        "AVG"
    } else if metric.default_agg == "NONE" {
        "NONE"
    } else {
        &metric.default_agg
    };

    // 4. 构造逻辑计划对应的 SQL 片段 (Select Clause)
    // 充分利用 sql_expression，支持 CASE WHEN 等复杂加工口径
    let select_item = if agg == "NONE" {
        format!("{} as \"{}\"", metric.sql_expression, metric.label)
    } else {
        format!("{}({}) as \"{}\"", agg, metric.sql_expression, metric.label)
    };

    // 5. 组装过滤条件与分组依据
    let mut where_conds = vec!["1=1".to_string()];
    let mut group_by_items = Vec::new();
    let mut select_items = vec![select_item];

    for (dim_node, val_code) in &filters {
        // A. 维度实例过滤：使用维度的物理表达式
        where_conds.push(format!("{} = '{}'", dim_node.sql_expression, val_code));

        // B. 维度回显：在结果中同时展示维度名
        select_items.insert(
            0,
            format!("{} as \"{}\"", dim_node.sql_expression, dim_node.label),
        );

        // C. 如果是聚合查询，需要加入 Group By
        if agg != "NONE" {
            group_by_items.push(dim_node.sql_expression.clone());
        }
    }

    // 6. 注入本体定义的业务隐含约束 (Implicit Constraints)
    // 规则：合并 [指标层约束] + [所有识别出的维度层约束]
    for c in &metric.default_constraints.0 {
        where_conds.push(format!("{} {} '{}'", c.column, c.operator, c.value));
    }
    for (dim_node, _) in &filters {
        for c in &dim_node.default_constraints.0 {
            where_conds.push(format!("{} {} '{}'", c.column, c.operator, c.value));
        }
    }

    // 7. 拼装完整物理 SQL
    let select_clause = select_items.join(", ");
    let where_clause = where_conds.join(" AND ");
    let mut sql = format!(
        "SELECT {} FROM {} WHERE {}",
        select_clause, metric.target_table, where_clause
    );

    if !group_by_items.is_empty() {
        sql.push_str(&format!(" GROUP BY {}", group_by_items.join(", ")));
    }

    info!("🚀 最终生成 SQL: {}", sql);

    // 8. 动态数据源路由与物理执行
    let source_res: Result<DataSource, _> =
        sqlx::query_as("SELECT * FROM data_sources WHERE id = $1")
            .bind(&metric.source_id)
            .fetch_one(&state.db)
            .await;

    let source = match source_res {
        Ok(s) => s,
        Err(_) => {
            return Json(json!({"status": "error", "message": "无法定位目标数据库配置"}))
                .into_response()
        }
    };

    let pool = match state.pool_manager.get_or_create_pool(&source).await {
        Ok(p) => p,
        Err(e) => {
            return Json(json!({"status": "error", "message": format!("数据库连接失败: {}", e)}))
                .into_response()
        }
    };

    let start_time = std::time::Instant::now();
    // 9. 执行并返回统一结构的结果集
    match &*pool {
        DynamicPool::Postgres(p) => {
            let rows_result = sqlx::query(&sql).fetch_all(p).await;
            match rows_result {
                Ok(rows) => {
                    let data: Vec<serde_json::Value> = rows.iter().map(pg_row_to_json).collect();
                    info!(
                        "✅ 执行成功 - 耗时: {:?}, 返回行数: {}",
                        start_time.elapsed(),
                        rows.len()
                    );
                    Json(json!({
                        "status": "success",
                        "sql": sql,
                        "logic": format!("指标: {}, 识别维度: {}个, 聚合: {}", metric.label, filters.len(), agg),
                        "data": data
                    })).into_response()
                }
                Err(e) => {
                    error!("Postgres 执行错误: {}", e);
                    Json(json!({"status": "error", "message": format!("Postgres 执行错误: {}", e)}))
                        .into_response()
                }
            }
        }
        DynamicPool::MySql(p) => {
            // MySQL 占位符兼容处理
            let sql_mysql = sql.replace("$1", "?");
            let rows_result = sqlx::query(&sql_mysql).fetch_all(p).await;
            match rows_result {
                Ok(rows) => {
                    let data: Vec<serde_json::Value> = rows.iter().map(mysql_row_to_json).collect();
                    Json(json!({
                        "status": "success",
                        "sql": sql_mysql,
                        "logic": format!("指标: {}, 识别维度: {}个, 聚合: {}", metric.label, filters.len(), agg),
                        "data": data
                    })).into_response()
                }
                Err(e) => {
                    Json(json!({"status": "error", "message": format!("MySQL 执行错误: {}", e)}))
                        .into_response()
                }
            }
        }
    }
}
