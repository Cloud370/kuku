use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Media output requested by an Agent or reusable model route.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestedOutput {
    /// Produce text.
    #[default]
    Text,
    /// Produce an image.
    Image,
}

/// Media supported by the built-in runtime adapters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaAdapterSupport {
    /// Built-in adapters currently send and receive text.
    #[default]
    TextOnly,
}

/// The scope of a capability value is the configured connection and model unless a protocol is named.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelCapabilityDeclaration {
    /// Native reasoning values reported or supplied for this model.
    pub reasoning_efforts: Option<Vec<String>>,
    /// Upstream reasoning default when reported or supplied.
    pub reasoning_default: Option<String>,
    /// Media accepted by the model.
    pub input_modalities: Option<Vec<String>>,
    /// Media produced by the model.
    pub output_modalities: Option<Vec<String>>,
    /// Interfaces advertised for the model.
    pub interfaces: Option<Vec<String>>,
    /// Capability overrides or metadata keyed by published protocol identifier.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub protocols: BTreeMap<String, ProtocolCapabilityDeclaration>,
}

/// Capabilities scoped to one protocol on a model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProtocolCapabilityDeclaration {
    /// Native reasoning values available on this protocol.
    pub reasoning_efforts: Option<Vec<String>>,
    /// Upstream reasoning default on this protocol.
    pub reasoning_default: Option<String>,
    /// Media accepted on this protocol.
    pub input_modalities: Option<Vec<String>>,
    /// Media produced on this protocol.
    pub output_modalities: Option<Vec<String>>,
    /// How the provider applies a changed system prompt within history.
    pub system_prompt_update: Option<String>,
}

/// Separate manual and provider declarations for one connection/model pair.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelCapabilityEntry {
    /// Latest declaration returned by the provider's model list.
    pub discovered: Option<ModelCapabilityDeclaration>,
    /// Explicit values supplied by the user and retained on refresh.
    pub manual: Option<ModelCapabilityDeclaration>,
    /// Thinking values tested through the configured generation endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_test: Option<ModelThinkingTest>,
}

/// An endpoint-scoped record of values the generation API accepted or rejected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelThinkingTest {
    pub scope: String,
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
}

impl ModelThinkingTest {
    /// Apply confirmed observations while leaving the existing wire options loadable.
    pub fn merge(&mut self, tested: Self) {
        if self.scope != tested.scope {
            *self = tested;
            return;
        }
        for value in tested.accepted {
            self.rejected.retain(|level| *level != value);
            if !self.accepted.contains(&value) {
                self.accepted.push(value);
            }
        }
        for value in tested.rejected {
            if !self.rejected.contains(&value) {
                self.rejected.push(value);
            }
        }
    }
}
