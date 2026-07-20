use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::api::{CatalogQuery, ContextCatalog, WorkspaceId};
use crate::context::read_model::ContextReadModel;

use crate::routes::context::{json_no_store, ContextHttpError};

pub(crate) fn router<S>(model: Arc<ContextReadModel>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/workspaces/{workspace_id}/catalog", get(get_catalog))
        .with_state(model)
        .layer(axum::middleware::map_response(
            crate::routes::context::no_store_response,
        ))
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawCatalogQuery {
    search: Option<String>,
}

pub(crate) async fn get_catalog(
    State(model): State<Arc<ContextReadModel>>,
    Path(workspace_id): Path<String>,
    Query(query): Query<RawCatalogQuery>,
) -> Result<Response, ContextHttpError> {
    let workspace_id = WorkspaceId::parse(workspace_id)
        .map_err(|_| crate::context::read_model::ContextReadError::InvalidRequest)?;
    let catalog: ContextCatalog = model.catalog(
        &workspace_id,
        CatalogQuery {
            search: query.search,
        },
    )?;
    Ok(json_no_store(catalog))
}
