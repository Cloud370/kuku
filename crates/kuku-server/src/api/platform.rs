use std::borrow::Cow;
use std::fmt;

use kuku::config::SecretString;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{ApiVersion, RevisionToken, WorkspaceId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationRootId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationRootIdError;

impl fmt::Display for RegistrationRootIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("registration root id must be root_ plus 24 lowercase hex characters")
    }
}

impl std::error::Error for RegistrationRootIdError {}

impl RegistrationRootId {
    pub fn try_new(value: impl Into<String>) -> Result<Self, RegistrationRootIdError> {
        let value = value.into();
        let suffix = value.strip_prefix("root_").unwrap_or_default();
        if suffix.len() == 24
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(value))
        } else {
            Err(RegistrationRootIdError)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for RegistrationRootId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RegistrationRootId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for RegistrationRootId {
    fn schema_name() -> Cow<'static, str> {
        "RegistrationRootId".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "string", "pattern": "^root_[0-9a-f]{24}$"})
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    LoopbackTrusted,
    Bearer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthStatus {
    pub authenticated: bool,
    pub mode: AuthMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectionInfo {
    pub server_id: String,
    pub display_name: String,
    pub preferred_origin: String,
    pub local_url: String,
    #[serde(deserialize_with = "required_nullable")]
    pub lan_url: Option<String>,
    pub plaintext: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InitPhase {
    Required,
    ProvidersReady,
    DefaultTierReady,
    WorkspaceReady,
    ProbePassed,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceAvailability {
    Available,
    Missing,
    Inaccessible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceSummary {
    pub workspace_id: WorkspaceId,
    pub label: String,
    pub is_default: bool,
    pub availability: WorkspaceAvailability,
    #[serde(deserialize_with = "required_nullable")]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegistrationRootSummary {
    pub root_id: RegistrationRootId,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegistrationRootPage {
    pub api_version: ApiVersion,
    pub items: Vec<RegistrationRootSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkspacePage {
    pub api_version: ApiVersion,
    pub server_revision: RevisionToken,
    pub items: Vec<WorkspaceSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisterWorkspaceRequest {
    pub root_id: RegistrationRootId,
    pub relative_path: String,
    pub label: String,
    pub expected_revision: RevisionToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoveWorkspaceRequest {
    pub expected_revision: RevisionToken,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "source", content = "value", rename_all = "snake_case")]
pub enum CredentialInput {
    DirectValue(#[schemars(with = "String")] SecretString),
    EnvironmentReference(String),
}

impl fmt::Debug for CredentialInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DirectValue(_) => formatter.write_str("DirectValue(<redacted>)"),
            Self::EnvironmentReference(value) => formatter
                .debug_tuple("EnvironmentReference")
                .field(value)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    DirectValue,
    EnvironmentReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialStatus {
    pub provider_id: String,
    pub present: bool,
    #[serde(deserialize_with = "required_nullable")]
    pub source: Option<CredentialSource>,
    #[serde(deserialize_with = "required_nullable")]
    pub environment_reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderDraft {
    pub provider_id: String,
    pub format: String,
    pub base_url: String,
    pub credential: CredentialInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TierDraft {
    pub tier_id: String,
    pub provider_id: String,
    pub model: String,
    pub purpose: String,
    #[serde(deserialize_with = "required_nullable")]
    pub think: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateProvidersRequest {
    pub providers: Vec<ProviderDraft>,
    pub tiers: Vec<TierDraft>,
    pub expected_revision: RevisionToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateDefaultTierRequest {
    pub tier_id: String,
    pub expected_revision: RevisionToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisterInitialWorkspaceRequest {
    pub workspace: RegisterWorkspaceRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TestProviderRequest {
    pub tier_id: String,
    pub expected_revision: RevisionToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TestProviderResult {
    pub api_version: ApiVersion,
    pub reachable: bool,
    pub provider: String,
    pub model: String,
    #[serde(deserialize_with = "required_nullable")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompleteInitRequest {
    pub expected_revision: RevisionToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InitStatus {
    pub api_version: ApiVersion,
    pub phase: InitPhase,
    pub server_revision: RevisionToken,
    pub providers_configured: bool,
    pub default_tier_configured: bool,
    pub workspace_registered: bool,
    pub provider_test_passed: bool,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlatformStatus {
    pub api_version: ApiVersion,
    pub ready: bool,
    pub auth: AuthStatus,
    pub init: InitStatus,
    pub connection: ConnectionInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingsSnapshot {
    pub api_version: ApiVersion,
    pub server_revision: RevisionToken,
    pub default_tier: String,
    pub credentials: Vec<CredentialStatus>,
    #[serde(deserialize_with = "required_nullable")]
    pub default_workspace_id: Option<WorkspaceId>,
    pub max_concurrent_runs: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SettingsPatch {
    #[serde(deserialize_with = "required_nullable")]
    pub default_tier: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub default_workspace_id: Option<WorkspaceId>,
    #[serde(deserialize_with = "required_nullable")]
    pub max_concurrent_runs: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateSettingsRequest {
    pub expected_revision: RevisionToken,
    pub patch: SettingsPatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlatformCatalog {
    pub api_version: ApiVersion,
    pub revision: RevisionToken,
    pub default_tier: super::TierSummary,
    pub tiers: Vec<super::TierSummary>,
    pub credentials: Vec<CredentialStatus>,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}
