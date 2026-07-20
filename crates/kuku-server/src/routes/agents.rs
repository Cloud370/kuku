use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;

use crate::api::{AgentThread, ConversationId, TaskId};
use crate::context::read_model::ContextReadModel;

use crate::routes::context::{json_no_store, ContextHttpError};

pub(crate) fn router<S>(model: Arc<ContextReadModel>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/tasks/{task_id}/agents/{conversation_id}", get(thread))
        .with_state(model)
        .layer(axum::middleware::map_response(
            crate::routes::context::no_store_response,
        ))
}

pub(crate) async fn thread(
    State(model): State<Arc<ContextReadModel>>,
    Path((task_id, conversation_id)): Path<(String, String)>,
) -> Result<Response, ContextHttpError> {
    let task_id = TaskId::parse(task_id)
        .map_err(|_| crate::context::read_model::ContextReadError::InvalidRequest)?;
    let conversation_id = ConversationId::parse(conversation_id)
        .map_err(|_| crate::context::read_model::ContextReadError::InvalidRequest)?;
    let thread: AgentThread = model.agent_thread(&task_id, &conversation_id)?;
    Ok(json_no_store(thread))
}
