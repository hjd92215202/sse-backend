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
use std::sync::Arc;
use uuid::Uuid;
use sqlx::Row;

// 保存本体节点及 RDF 指标-维度关系
pub async fn save_mapping(State(state): State<Arc<AppState>>, Json(payload): Json<CreateNodeRequest>) -> impl IntoResponse {
    let mut tx = state.db.begin().await.unwrap();

    // 1. 更新 ontology_nodes
    let node_id = match sqlx::query!(
        "INSERT INTO ontology_nodes (node_key, label, node_role) VALUES ($1, $2, $3) 
         ON CONFLICT (node_key) DO UPDATE SET label = EXCLUDED.label, node_role = EXCLUDED.node_role RETURNING id",
        payload.node_key, payload.label, payload.node_role
    ).fetch_one(&mut *tx).await {
        Ok(rec) => rec.id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Node Error: {}", e)).into_response(),
    };

    // 2. 更新 semantic_definitions
    let constraints_json = serde_json::to_value(&payload.default_constraints).unwrap();
    if let Err(e) = sqlx::query!(
        r#"
        INSERT INTO semantic_definitions (node_id, source_id, target_table, target_column, default_constraints, alias_names, default_agg)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (node_id) DO UPDATE SET 
            source_id = EXCLUDED.source_id,
            target_table = EXCLUDED.target_table,
            target_column = EXCLUDED.target_column,
            default_constraints = EXCLUDED.default_constraints,
            alias_names = EXCLUDED.alias_names,
            default_agg = EXCLUDED.default_agg
        "#,
        node_id, payload.source_id, payload.target_table, payload.target_column, constraints_json, &payload.alias_names, payload.default_agg
    ).execute(&mut *tx).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("Definition Error: {}", e)).into_response();
    }

    // 3. 更新指标与维度的 RDF 关系
    let _ = sqlx::query!("DELETE FROM metric_dimension_rels WHERE metric_node_id = $1", node_id).execute(&mut *tx).await;
    if payload.node_role == "METRIC" {
        for dim_id in payload.supported_dimension_ids {
            let _ = sqlx::query!(
                "INSERT INTO metric_dimension_rels (metric_node_id, dimension_node_id) VALUES ($1, $2)",
                node_id, dim_id
            ).execute(&mut *tx).await;
        }
    }

    tx.commit().await.unwrap();
    let _ = refresh_fst_cache(&state).await;
    (StatusCode::OK, Json(serde_json::json!({ "id": node_id }))).into_response()
}

pub async fn list_mappings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // 关键修复：补全了 d.default_agg 字段，确保与 FullSemanticNode 结构体匹配
    let rows = sqlx::query_as::<_, FullSemanticNode>(
        r#"
        SELECT n.id, n.node_key, n.label, n.node_role, d.source_id, d.target_table, d.target_column, 
               d.default_constraints, d.alias_names, d.default_agg,
               COALESCE(array_agg(r.dimension_node_id) FILTER (WHERE r.dimension_node_id IS NOT NULL), '{}') as supported_dimension_ids
        FROM ontology_nodes n 
        JOIN semantic_definitions d ON n.id = d.node_id
        LEFT JOIN metric_dimension_rels r ON n.id = r.metric_node_id
        GROUP BY n.id, n.node_key, n.label, n.node_role, d.source_id, d.target_table, d.target_column, d.default_constraints, d.alias_names, d.default_agg
        "#
    ).fetch_all(&state.db).await;

    match rows {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("List Query Error: {}", e)).into_response(),
    }
}

pub async fn export_ontology_ttl(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let nodes = sqlx::query_as::<_, FullSemanticNode>(
        r#"
        SELECT n.id, n.node_key, n.label, n.node_role, d.source_id, d.target_table, d.target_column, 
               d.default_constraints, d.alias_names, d.default_agg,
               COALESCE(array_agg(r.dimension_node_id) FILTER (WHERE r.dimension_node_id IS NOT NULL), '{}') as supported_dimension_ids
        FROM ontology_nodes n 
        JOIN semantic_definitions d ON n.id = d.node_id
        LEFT JOIN metric_dimension_rels r ON n.id = r.metric_node_id
        GROUP BY n.id, n.node_key, n.label, n.node_role, d.source_id, d.target_table, d.target_column, d.default_constraints, d.alias_names, d.default_agg
        "#
    ).fetch_all(&state.db).await.unwrap_or_default();

    let id_to_key_map: std::collections::HashMap<uuid::Uuid, String> = nodes.iter()
        .map(|n| (n.id, n.node_key.clone()))
        .collect();

    let mut ttl = String::from("@prefix sse: <http://example.org/sse#> .\n");
    ttl.push_str("@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n");
    ttl.push_str("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\n");

    for n in nodes {
        let uri = format!("sse:{}", n.node_key);
        let n_type = if n.node_role == "METRIC" { "sse:Metric" } else { "sse:Dimension" };
        ttl.push_str(&format!("{} rdf:type {} ;\n", uri, n_type));
        ttl.push_str(&format!("    rdfs:label \"{}\" ;\n", n.label));
        ttl.push_str(&format!("    sse:targetTable \"{}\" ;\n", n.target_table));
        ttl.push_str(&format!("    sse:targetColumn \"{}\" ;\n", n.target_column));
        ttl.push_str(&format!("    sse:defaultAgg \"{}\" ;\n", n.default_agg));

        if n.node_role == "METRIC" && !n.supported_dimension_ids.is_empty() {
            for dim_id in &n.supported_dimension_ids {
                if let Some(dim_key) = id_to_key_map.get(dim_id) {
                    ttl.push_str(&format!("    sse:hasDimension sse:{} ;\n", dim_key));
                }
            }
        }
        if !n.alias_names.is_empty() {
            let aliases = n.alias_names.join("\", \"");
            ttl.push_str(&format!("    sse:alias \"{}\" ;\n", aliases));
        }
        ttl.push_str(&format!("    sse:systemId \"{}\" .\n\n", n.id));
    }

    Response::builder()
        .header(header::CONTENT_TYPE, "text/turtle")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"sse_ontology.ttl\"")
        .body(axum::body::Body::from(ttl))
        .unwrap()
}

pub async fn get_metadata_tables(State(state): State<Arc<AppState>>, Query(req): Query<MetadataRequest>) -> impl IntoResponse {
    let source = sqlx::query_as::<_, DataSource>("SELECT * FROM data_sources WHERE id = $1").bind(&req.source_id).fetch_one(&state.db).await;
    match source {
        Ok(s) => Json(state.pool_manager.list_tables(&s).await.unwrap_or_default()).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response()
    }
}

pub async fn get_metadata_columns(State(state): State<Arc<AppState>>, Query(req): Query<MetadataRequest>) -> impl IntoResponse {
    let source = sqlx::query_as::<_, DataSource>("SELECT * FROM data_sources WHERE id = $1").bind(&req.source_id).fetch_one(&state.db).await;
    match source {
        Ok(s) => Json(state.pool_manager.list_columns(&s, &req.table_name.unwrap_or_default()).await.unwrap_or_default()).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response()
    }
}

pub async fn sync_dimension_values(State(state): State<Arc<AppState>>, Path(node_id): Path<Uuid>) -> impl IntoResponse {
    let node_info = match sqlx::query!("SELECT source_id, target_table, target_column FROM semantic_definitions WHERE node_id = $1", node_id).fetch_one(&state.db).await {
        Ok(n) => n,
        Err(_) => return (StatusCode::NOT_FOUND, "Node not found").into_response(),
    };
    let source = sqlx::query_as::<_, DataSource>("SELECT * FROM data_sources WHERE id = $1").bind(&node_info.source_id).fetch_one(&state.db).await.unwrap();
    let pool = state.pool_manager.get_or_create_pool(&source).await.unwrap();
    let sql = format!("SELECT DISTINCT {}::text FROM {}", node_info.target_column, node_info.target_table);
    let rows = match &*pool {
        crate::infra::db_external::DynamicPool::Postgres(p) => sqlx::query(&sql).fetch_all(p).await.unwrap().into_iter().filter_map(|r| r.try_get::<String, _>(0).ok()).collect::<Vec<_>>(),
        _ => vec![]
    };
    for v in rows {
        let _ = sqlx::query!("INSERT INTO dimension_values (dimension_node_id, value_label, value_code) VALUES ($1, $2, $2) ON CONFLICT DO NOTHING", node_id, v).execute(&state.db).await;
    }
    (StatusCode::OK, "Sync Complete").into_response()
}

pub async fn register_data_source(State(state): State<Arc<AppState>>, Json(payload): Json<CreateDataSourceRequest>) -> impl IntoResponse {
    let res = sqlx::query("INSERT INTO data_sources (id, db_type, connection_url, display_name) VALUES ($1, $2, $3, $4) ON CONFLICT (id) DO UPDATE SET connection_url=EXCLUDED.connection_url, display_name=EXCLUDED.display_name")
    .bind(&payload.id).bind(&payload.db_type).bind(&payload.connection_url).bind(&payload.display_name).execute(&state.db).await;
    match res {
        Ok(_) => (StatusCode::CREATED, "Source Registered").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn list_data_sources(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rows = sqlx::query_as::<_, DataSource>("SELECT id, db_type, connection_url, display_name FROM data_sources").fetch_all(&state.db).await;
    match rows {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn refresh_fst_cache(state: &AppState) -> anyhow::Result<()> {
    let nodes = sqlx::query_as::<_, FullSemanticNode>(
        "SELECT n.id, n.node_key, n.label, n.node_role, d.source_id, d.target_table, d.target_column, d.default_constraints, d.alias_names, d.default_agg FROM ontology_nodes n JOIN semantic_definitions d ON n.id = d.node_id"
    ).fetch_all(&state.db).await?;
    let new_engine = FstEngine::build(&nodes)?;
    let mut guard = state.fst.write().await;
    *guard = new_engine;
    Ok(())
}