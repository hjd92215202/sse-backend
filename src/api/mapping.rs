use crate::ax_state::AppState;
use crate::core::fst_engine::FstEngine;
use crate::models::schema::{
    CreateDataSourceRequest, CreateNodeRequest, DataSource, FullSemanticNode, MetadataRequest,
};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use sqlx::{Postgres, Row};
use std::sync::Arc;
use tracing::{info};
use uuid::Uuid;

// --- 1. 本体节点建模与管理 ---

/// 保存或更新本体节点 (Metric/Dimension)
/// 处理流程：开启事务 -> 更新主表 -> 更新定义表 -> 重置 T-Box 关系 -> 提交 -> 刷新 FST
pub async fn save_mapping(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateNodeRequest>,
) -> impl IntoResponse {
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    info!(
        "接收到建模请求: node_key={}, role={}",
        payload.node_key, payload.node_role
    );

    // A. 更新 ontology_nodes (核心信息)
    let node_id: Uuid = match sqlx::query(
        "INSERT INTO ontology_nodes (node_key, label, node_role, semantic_type, dataset_id) 
         VALUES ($1, $2, $3, $4, $5) 
         ON CONFLICT (node_key) 
         DO UPDATE SET label = EXCLUDED.label, node_role = EXCLUDED.node_role, semantic_type=EXCLUDED.semantic_type, dataset_id = EXCLUDED.dataset_id 
         RETURNING id"
    )
    .bind(&payload.node_key)
    .bind(&payload.label)
    .bind(&payload.node_role)
    .bind(&payload.semantic_type)
    .bind(payload.dataset_id)
    .fetch_one(&mut *tx).await {
        Ok(row) => row.get("id"),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Ontology Update Failed: {}", e)).into_response(),
    };

    // B. 更新 semantic_definitions (物理映射、SQL表达式、默认聚合、隐含约束)
    let constraints_json = serde_json::to_value(&payload.default_constraints).unwrap();
    let def_res = sqlx::query(
        r#"
        INSERT INTO semantic_definitions (node_id, source_id, target_table, sql_expression, default_constraints, alias_names, default_agg, value_format)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8) 
        ON CONFLICT (node_id) 
        DO UPDATE SET 
            source_id = EXCLUDED.source_id, 
            target_table = EXCLUDED.target_table, 
            sql_expression = EXCLUDED.sql_expression, 
            default_constraints = EXCLUDED.default_constraints, 
            alias_names = EXCLUDED.alias_names, 
            default_agg = EXCLUDED.default_agg, 
            value_format = EXCLUDED.value_format
        "#
    )
    .bind(node_id)
    .bind(&payload.source_id)
    .bind(&payload.target_table)
    .bind(&payload.sql_expression)
    .bind(constraints_json)
    .bind(&payload.alias_names)
    .bind(&payload.default_agg)
    .bind(&payload.value_format)
    .execute(&mut *tx).await;

    if let Err(e) = def_res {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Mapping Definition Failed: {}", e),
        )
            .into_response();
    }

    // C. 更新 T-Box 维度关联关系 (只有指标角色需要)
    let _ = sqlx::query("DELETE FROM metric_dimension_rels WHERE metric_node_id = $1")
        .bind(node_id)
        .execute(&mut *tx)
        .await;

    if payload.node_role == "METRIC" {
        for dim_id in payload.supported_dimension_ids {
            let _ = sqlx::query(
                "INSERT INTO metric_dimension_rels (metric_node_id, dimension_node_id) VALUES ($1, $2)"
            )
            .bind(node_id)
            .bind(dim_id)
            .execute(&mut *tx).await;
        }
    }

    if let Err(e) = tx.commit().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Transaction Commit Failed: {}", e),
        )
            .into_response();
    }

    info!("建模请求处理完成: node_id={}", node_id);

    // 热刷新内存中的语义索引
    let _ = full_reload_semantic_engine(&state).await;

    (StatusCode::OK, Json(serde_json::json!({ "id": node_id }))).into_response()
}

/// 删除本体节点
pub async fn delete_mapping(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    // 依赖数据库设置的 ON DELETE CASCADE 自动清理物理定义与关联
    match sqlx::query("DELETE FROM ontology_nodes WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(_) => {
            let _ = refresh_fst_cache(&state).await;
            info!("删除语义节点: id={}", id);
            StatusCode::OK.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// 获取全量本体节点列表 (用于前端表格展示，包含维度 ID 聚合)
pub async fn list_mappings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rows = sqlx::query_as::<Postgres, FullSemanticNode>(
        r#"
        SELECT n.id, n.node_key, n.label, n.node_role, n.semantic_type, d.source_id, d.target_table, d.sql_expression, 
               d.default_constraints, d.alias_names, d.default_agg, n.dataset_id, d.value_format,
               COALESCE(array_agg(r.dimension_node_id) FILTER (WHERE r.dimension_node_id IS NOT NULL), '{}') as supported_dimension_ids
        FROM ontology_nodes n 
        JOIN semantic_definitions d ON n.id = d.node_id
        LEFT JOIN metric_dimension_rels r ON n.id = r.metric_node_id
        GROUP BY n.id, n.node_key, n.label, n.node_role, n.semantic_type, d.source_id, d.target_table, d.sql_expression, d.default_constraints, d.alias_names, d.default_agg, n.dataset_id, d.value_format
        "#
    ).fetch_all(&state.db).await;

    match rows {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// --- 2. 物理元数据探测与 A-Box 同步 ---

/// 获取外部数据库的表列表 (级联第一步)
pub async fn get_metadata_tables(
    State(state): State<Arc<AppState>>,
    Query(req): Query<MetadataRequest>,
) -> impl IntoResponse {
    let source = sqlx::query_as::<Postgres, DataSource>("SELECT * FROM data_sources WHERE id = $1")
        .bind(&req.source_id)
        .fetch_one(&state.db)
        .await;
    match source {
        Ok(s) => Json(state.pool_manager.list_tables(&s).await.unwrap_or_default()).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Source config not found").into_response(),
    }
}

/// 获取外部表的列列表 (级联第二步)
pub async fn get_metadata_columns(
    State(state): State<Arc<AppState>>,
    Query(req): Query<MetadataRequest>,
) -> impl IntoResponse {
    let source = sqlx::query_as::<Postgres, DataSource>("SELECT * FROM data_sources WHERE id = $1")
        .bind(&req.source_id)
        .fetch_one(&state.db)
        .await;
    match source {
        Ok(s) => Json(
            state
                .pool_manager
                .list_columns(&s, &req.table_name.unwrap_or_default())
                .await
                .unwrap_or_default(),
        )
        .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Source config not found").into_response(),
    }
}

/// 同步维度码值 (将物理数据值拉入 A-Box 语义存储)
pub async fn sync_dimension_values(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<Uuid>,
) -> impl IntoResponse {
    let def_row = sqlx::query("SELECT d.source_id, d.target_table, d.sql_expression FROM semantic_definitions d WHERE d.node_id = $1")
        .bind(node_id)
        .fetch_one(&state.db).await;

    let info = match def_row {
        Ok(r) => r,
        Err(_) => return (StatusCode::NOT_FOUND, "Ontology Definition missing").into_response(),
    };

    let source_id: String = info.get("source_id");
    let target_table: String = info.get("target_table");
    let sql_expression: String = info.get("sql_expression");

    let source = sqlx::query_as::<Postgres, DataSource>("SELECT * FROM data_sources WHERE id = $1")
        .bind(&source_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

    let pool = state
        .pool_manager
        .get_or_create_pool(&source)
        .await
        .unwrap();

    // 执行基于逻辑表达式的去重查询
    let sql = format!(
        "SELECT DISTINCT ({}) :: text as val FROM {}",
        sql_expression, target_table
    );
    info!("开始 A-Box 同步，物理查询: {}", sql);
    let vals = match &*pool {
        crate::infra::db_external::DynamicPool::Postgres(p) => sqlx::query(&sql)
            .fetch_all(p)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|r| r.try_get::<String, _>("val").ok())
            .collect::<Vec<_>>(),
        _ => vec![],
    };

    let count = vals.len();

    for v in vals {
        // 使用 bind 模式防止宏解析错误，保存实例数据
        let _ = sqlx::query("INSERT INTO dimension_values (dimension_node_id, value_label, value_code) VALUES ($1, $2, $2) ON CONFLICT DO NOTHING")
            .bind(node_id)
            .bind(v)
            .execute(&state.db).await;
    }
    info!("A-Box 同步完成，新增/更新 {} 个实例", count);

    let _ = full_reload_semantic_engine(&state).await;
    (StatusCode::OK, "A-Box Synced Successfully").into_response()
}

// --- 3. 语义资产导出 ---

/// 导出本体知识库为标准 TTL (Turtle) 格式
pub async fn export_ontology_ttl(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let nodes = sqlx::query_as::<Postgres, FullSemanticNode>(
        "SELECT n.id, n.node_key, n.label, n.node_role, d.source_id, d.target_table, d.sql_expression, 
                d.default_constraints, d.alias_names, d.default_agg, n.dataset_id,
         COALESCE(array_agg(r.dimension_node_id) FILTER (WHERE r.dimension_node_id IS NOT NULL), '{}') as supported_dimension_ids
         FROM ontology_nodes n JOIN semantic_definitions d ON n.id = d.node_id LEFT JOIN metric_dimension_rels r ON n.id = r.metric_node_id
         GROUP BY n.id, n.node_key, n.label, n.node_role, d.source_id, d.target_table, d.sql_expression, d.default_constraints, d.alias_names, d.default_agg, n.dataset_id"
    ).fetch_all(&state.db).await.unwrap_or_default();

    // 建立 ID 到语义 Key 的映射，用于 RDF 指向
    let id_to_key_map: std::collections::HashMap<Uuid, String> =
        nodes.iter().map(|n| (n.id, n.node_key.clone())).collect();
    let mut ttl = String::from("@prefix sse: <http://example.org/sse#> .\n@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\n");

    for n in nodes {
        let uri = format!("sse:{}", n.node_key);
        let role_iri = if n.node_role == "METRIC" {
            "sse:Metric"
        } else {
            "sse:Dimension"
        };

        ttl.push_str(&format!("{} rdf:type {} ;\n", uri, role_iri));
        ttl.push_str(&format!("    rdfs:label \"{}\" ;\n", n.label));
        ttl.push_str(&format!("    sse:physicalTable \"{}\" ;\n", n.target_table));
        ttl.push_str(&format!(
            "    sse:sqlExpression \"{}\" ;\n",
            n.sql_expression
        ));

        // 导出 T-Box 关联：使用语义标识符而非 UUID
        if n.node_role == "METRIC" {
            for dim_id in &n.supported_dimension_ids {
                if let Some(dim_key) = id_to_key_map.get(dim_id) {
                    ttl.push_str(&format!("    sse:hasDimension sse:{} ;\n", dim_key));
                }
            }
        }
        ttl.push_str(&format!("    sse:systemId \"{}\" .\n\n", n.id));
    }

    info!("语义 TTL 导出完成");

    Response::builder()
        .header(header::CONTENT_TYPE, "text/turtle")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"sse_enterprise_ontology.ttl\"",
        )
        .body(axum::body::Body::from(ttl))
        .unwrap()
}

// --- 4. 数据源基础管理 ---

pub async fn register_data_source(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateDataSourceRequest>,
) -> impl IntoResponse {
    let res = sqlx::query("INSERT INTO data_sources (id, db_type, connection_url, display_name) VALUES ($1, $2, $3, $4) ON CONFLICT (id) DO UPDATE SET connection_url=EXCLUDED.connection_url, display_name=EXCLUDED.display_name")
    .bind(&payload.id).bind(&payload.db_type).bind(&payload.connection_url).bind(&payload.display_name).execute(&state.db).await;
    match res {
        Ok(_) => {
            info!("数据源配置已更新: id={}", payload.id);
            (StatusCode::CREATED, "Source Registered").into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn list_data_sources(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rows = sqlx::query_as::<Postgres, DataSource>(
        "SELECT id, db_type, connection_url, display_name FROM data_sources",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// 内部辅助：热重载内存语义索引
async fn refresh_fst_cache(state: &AppState) -> anyhow::Result<()> {
    let nodes = sqlx::query_as::<Postgres, FullSemanticNode>(
        "SELECT n.id, n.node_key, n.label, n.node_role, n.semantic_type, d.source_id, d.target_table, d.sql_expression, 
                d.default_constraints, d.alias_names, d.default_agg, d.value_format, n.dataset_id, 
                '{}'::uuid[] as supported_dimension_ids 
         FROM ontology_nodes n 
         JOIN semantic_definitions d ON n.id = d.node_id"
    ).fetch_all(&state.db).await?;
    let mut guard = state.fst.write().await;
    *guard = FstEngine::build(&nodes)?;
    info!("内存语义索引 FST 已热刷新");
    Ok(())
}

async fn full_reload_semantic_engine(state: &AppState) -> anyhow::Result<()> {
    let nodes = sqlx::query_as::<Postgres, FullSemanticNode>(
        "SELECT n.id, n.node_key, n.label, n.node_role, n.semantic_type, d.source_id, d.target_table, d.sql_expression, d.default_constraints, d.alias_names, d.default_agg, n.dataset_id, d.value_format,
        '{}'::uuid[] as supported_dimension_ids FROM ontology_nodes n JOIN semantic_definitions d ON n.id = d.node_id"
    ).fetch_all(&state.db).await?;

    // 1. 刷新 FST
    {
        let mut fst_guard = state.fst.write().await;
        *fst_guard = FstEngine::build(&nodes)?;
    }

    // 2. 刷新 Jieba
    {
        let mut engine_guard = state.engine.write().await;
        let mut words = nodes.iter().flat_map(|n| {
            let mut v = vec![n.label.clone()];
            v.extend(n.alias_names.clone());
            v
        }).collect::<Vec<String>>();
        
        let codes = sqlx::query("SELECT value_label FROM dimension_values").fetch_all(&state.db).await?;
        words.extend(codes.into_iter().map(|r| r.get::<String, _>(0)));
        
        engine_guard.refresh_custom_words(words);
    }
    Ok(())
}