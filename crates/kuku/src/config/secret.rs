use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// A secret value that is redacted from debug and display output.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wrap a value that must not appear in debug or display output.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Explicitly expose the secret value for an authorized consumer.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(<redacted>)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

impl Serialize for SecretString {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.expose())
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::new)
    }
}

/// A persisted credential with an explicit direct or environment source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", content = "value", rename_all = "snake_case")]
pub enum StoredCredential {
    /// A credential stored directly in private configuration.
    DirectValue(SecretString),
    /// The name of an environment variable containing the credential.
    EnvironmentReference(String),
}

impl StoredCredential {
    /// Resolve the stored source into a secret value.
    pub fn resolve(&self) -> Result<SecretString> {
        match self {
            StoredCredential::DirectValue(value) => Ok(value.clone()),
            StoredCredential::EnvironmentReference(name) => {
                std::env::var(name).map(SecretString::new).map_err(|_| {
                    Error::ConfigLoad(format!(
                        "env var '{name}' referenced by credential is not set"
                    ))
                })
            }
        }
    }
}
