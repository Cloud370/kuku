use std::path::{Path, PathBuf};

use kuku::config::ProviderFormat;
use kuku::{ProviderCapabilities, ProviderSettings};
use serde::{Deserialize, Serialize};

use crate::{AgentConfig, ApiError, Client, Error, ModelSelection};

/// The server's configured routes for model selection.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelCatalog {
    /// Configured default tier.
    pub default_tier: String,
    /// Named connections available on the service host.
    pub connections: Vec<ConnectionSummary>,
    /// Configured reusable model routes.
    pub tiers: Vec<TierSummary>,
}

/// Connection metadata safe to share without logging provider credentials.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ConnectionSummary {
    /// Configured connection name.
    pub name: String,
    /// Wire protocol used by this connection.
    pub protocol: ProviderFormat,
    /// Subscription channel and account label with masked identity, when applicable.
    pub subscription_label: Option<String>,
}

/// A named model route with its effective settings and supported options.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TierSummary {
    /// Configured tier name.
    pub name: String,
    /// Named connection selected for the tier.
    pub connection: String,
    /// Model identifier selected for the tier.
    pub model: String,
    /// Configured user-facing purpose, when available.
    pub purpose: Option<String>,
    /// Effective provider settings, when resolution succeeds.
    pub settings: Option<ProviderSettings>,
    /// Media output inherited from this route.
    pub output: Option<kuku::config::RequestedOutput>,
    /// Resolved request destination.
    pub request_url: Option<String>,
    /// Declared supported provider options, when resolution succeeds.
    pub capabilities: Option<ProviderCapabilities>,
    /// Service error when the configured route cannot be resolved.
    pub error: Option<ApiError>,
}

/// A discoverable Skill in the selected workspace.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SkillSummary {
    /// Skill name used to select its instructions.
    pub name: String,
    /// Short description supplied by the Skill.
    pub description: String,
}

impl Client {
    /// List model routes and available provider settings from the service.
    pub async fn models(&self) -> Result<ModelCatalog, Error> {
        self.get(["models"], &()).await
    }

    /// Resolve the proposed model and settings on the service host.
    pub async fn resolve_model(&self, config: &AgentConfig) -> Result<ModelSelection, Error> {
        #[derive(Serialize)]
        struct Query<'a> {
            tier: Option<&'a str>,
            connection: Option<&'a str>,
            model: Option<&'a str>,
            reasoning_effort: Option<&'a str>,
            output: Option<kuku::config::RequestedOutput>,
            service_tier: Option<&'a str>,
            max_output_tokens: Option<u32>,
        }
        #[derive(Deserialize)]
        struct Model {
            selection: ModelSelection,
        }
        let response: Model = self
            .get(
                ["models", "resolve"],
                &Query {
                    tier: config.tier.as_deref(),
                    connection: config.connection.as_deref(),
                    model: config.model.as_deref(),
                    reasoning_effort: config.reasoning_effort.as_deref(),
                    output: config.output,
                    service_tier: config.service_tier.as_deref(),
                    max_output_tokens: config.max_output_tokens,
                },
            )
            .await?;
        Ok(response.selection)
    }

    /// List workspace paths visible to this client.
    pub async fn workspaces(&self) -> Result<Vec<PathBuf>, Error> {
        #[derive(Deserialize)]
        struct Workspaces {
            workspaces: Vec<PathBuf>,
        }
        let response: Workspaces = self.get(["workspaces"], &()).await?;
        Ok(response.workspaces)
    }

    /// Find allowed workspace-relative paths for completion.
    pub async fn workspace_files(
        &self,
        workspace: &Path,
        query: &str,
    ) -> Result<Vec<String>, Error> {
        #[derive(Serialize)]
        struct Query<'a> {
            workspace: &'a Path,
            query: &'a str,
        }
        #[derive(Deserialize)]
        struct Files {
            paths: Vec<String>,
        }
        let response: Files = self
            .get(["workspaces", "files"], &Query { workspace, query })
            .await?;
        Ok(response.paths)
    }

    /// Find Skills that are visible in the selected workspace.
    pub async fn skills(
        &self,
        workspace: &Path,
        query: Option<&str>,
    ) -> Result<Vec<SkillSummary>, Error> {
        #[derive(Serialize)]
        struct Query<'a> {
            workspace: &'a Path,
            query: Option<&'a str>,
        }
        #[derive(Deserialize)]
        struct Skills {
            skills: Vec<SkillSummary>,
        }
        let response: Skills = self
            .get(["workspaces", "skills"], &Query { workspace, query })
            .await?;
        Ok(response.skills)
    }
}
