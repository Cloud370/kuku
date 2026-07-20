use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};

use crate::api::{
    ApiError, ApiErrorCode, CompleteInitRequest, ConnectionInfo, PlatformStatus,
    RegisterInitialWorkspaceRequest, RegisterWorkspaceRequest, RemoveWorkspaceRequest,
    TestProviderRequest, UpdateDefaultTierRequest, UpdateProvidersRequest, UpdateSettingsRequest,
    WorkspaceId,
};

use super::{
    AuthContext, BearerTokenStore, BootstrapService, ConfigService, OriginPolicy, SettingsService,
    WorkspaceRegistry,
};

#[derive(Clone)]
pub struct PlatformServices {
    pub bootstrap: Arc<BootstrapService>,
    pub config: Arc<ConfigService>,
    pub settings: Arc<SettingsService>,
    pub workspaces: Arc<WorkspaceRegistry>,
    pub auth: Arc<BearerTokenStore>,
    pub origin_policy: Arc<OriginPolicy>,
    pub connection: ConnectionInfo,
}

impl PlatformServices {
    pub async fn status(&self, auth: &AuthContext) -> PlatformStatus {
        let init = self.bootstrap.status().await;
        PlatformStatus {
            api_version: crate::api::ApiVersion,
            ready: init.complete,
            auth: self.auth.status(auth),
            init,
            connection: self.connection.clone(),
        }
    }
}

pub fn router(state: Arc<PlatformServices>) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/init/status", get(init_status))
        .route("/init/providers", post(init_providers))
        .route("/init/default-tier", post(init_default_tier))
        .route("/init/workspace", post(init_workspace))
        .route("/init/test", post(init_test))
        .route("/init/complete", post(init_complete))
        .route("/settings", get(settings).patch(patch_settings))
        .route("/registration-roots", get(registration_roots))
        .route("/workspaces", get(workspaces).post(register_workspace))
        .route("/workspaces/{workspace_id}", delete(remove_workspace))
        .route("/catalog", get(catalog))
        .with_state(state)
}

async fn status(
    State(state): State<Arc<PlatformServices>>,
    Extension(auth): Extension<AuthContext>,
) -> Json<PlatformStatus> {
    Json(state.status(&auth).await)
}

async fn init_status(State(state): State<Arc<PlatformServices>>) -> impl IntoResponse {
    Json(state.bootstrap.status().await)
}

async fn init_providers(
    State(state): State<Arc<PlatformServices>>,
    Json(request): Json<UpdateProvidersRequest>,
) -> Result<impl IntoResponse, PlatformHttpError> {
    Ok(Json(state.bootstrap.update_providers(request).await?))
}

async fn init_default_tier(
    State(state): State<Arc<PlatformServices>>,
    Json(request): Json<UpdateDefaultTierRequest>,
) -> Result<impl IntoResponse, PlatformHttpError> {
    Ok(Json(state.bootstrap.update_default_tier(request).await?))
}

async fn init_workspace(
    State(state): State<Arc<PlatformServices>>,
    Json(request): Json<RegisterInitialWorkspaceRequest>,
) -> Result<impl IntoResponse, PlatformHttpError> {
    Ok(Json(
        state.bootstrap.register_initial_workspace(request).await?,
    ))
}

async fn init_test(
    State(state): State<Arc<PlatformServices>>,
    Json(request): Json<TestProviderRequest>,
) -> Result<impl IntoResponse, PlatformHttpError> {
    Ok(Json(state.bootstrap.test_provider(request).await?))
}

async fn init_complete(
    State(state): State<Arc<PlatformServices>>,
    Json(request): Json<CompleteInitRequest>,
) -> Result<impl IntoResponse, PlatformHttpError> {
    Ok(Json(state.bootstrap.complete(request).await?))
}

async fn settings(
    State(state): State<Arc<PlatformServices>>,
) -> Result<impl IntoResponse, PlatformHttpError> {
    Ok(Json(state.settings.snapshot().await?))
}

async fn patch_settings(
    State(state): State<Arc<PlatformServices>>,
    Json(request): Json<UpdateSettingsRequest>,
) -> Result<impl IntoResponse, PlatformHttpError> {
    Ok(Json(state.settings.commit(request).await?))
}

async fn registration_roots(State(state): State<Arc<PlatformServices>>) -> impl IntoResponse {
    Json(state.bootstrap.registration_roots())
}

async fn workspaces(
    State(state): State<Arc<PlatformServices>>,
) -> Result<impl IntoResponse, PlatformHttpError> {
    Ok(Json(state.workspaces.list().await?))
}

async fn register_workspace(
    State(state): State<Arc<PlatformServices>>,
    Json(request): Json<RegisterWorkspaceRequest>,
) -> Result<impl IntoResponse, PlatformHttpError> {
    Ok(Json(state.workspaces.register(request).await?))
}

async fn remove_workspace(
    State(state): State<Arc<PlatformServices>>,
    Path(workspace_id): Path<WorkspaceId>,
    Json(request): Json<RemoveWorkspaceRequest>,
) -> Result<StatusCode, PlatformHttpError> {
    state.workspaces.remove(&workspace_id, request).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn catalog(State(state): State<Arc<PlatformServices>>) -> impl IntoResponse {
    Json(state.config.catalog().await)
}

struct PlatformHttpError(ApiError);

impl From<ApiError> for PlatformHttpError {
    fn from(error: ApiError) -> Self {
        Self(error)
    }
}

impl IntoResponse for PlatformHttpError {
    fn into_response(self) -> Response {
        (status_for(self.0.code()), Json(self.0)).into_response()
    }
}

fn status_for(code: ApiErrorCode) -> StatusCode {
    match code {
        ApiErrorCode::AuthRequired => StatusCode::UNAUTHORIZED,
        ApiErrorCode::Forbidden | ApiErrorCode::OriginNotAllowed => StatusCode::FORBIDDEN,
        ApiErrorCode::WorkspaceNotFound
        | ApiErrorCode::TaskNotFound
        | ApiErrorCode::RequestNotFound
        | ApiErrorCode::ConversationNotFound
        | ApiErrorCode::FileNotFound => StatusCode::NOT_FOUND,
        ApiErrorCode::InitIncomplete
        | ApiErrorCode::StaleServerRevision
        | ApiErrorCode::WorkspaceInUse
        | ApiErrorCode::TaskBusy
        | ApiErrorCode::StaleCommand
        | ApiErrorCode::IdempotencyConflict
        | ApiErrorCode::RunNotActive
        | ApiErrorCode::InteractionNotPending
        | ApiErrorCode::CursorAhead
        | ApiErrorCode::Outdated => StatusCode::CONFLICT,
        ApiErrorCode::ProviderUnavailable => StatusCode::BAD_GATEWAY,
        ApiErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ApiErrorCode::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ApiErrorCode::ServerBusy | ApiErrorCode::StreamLimit => StatusCode::TOO_MANY_REQUESTS,
        ApiErrorCode::StorageExhausted => StatusCode::INSUFFICIENT_STORAGE,
        ApiErrorCode::InvalidRequest | ApiErrorCode::WorkspaceUnavailable => {
            StatusCode::BAD_REQUEST
        }
        ApiErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
