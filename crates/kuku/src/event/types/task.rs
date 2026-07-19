use std::borrow::Cow;
use std::fmt;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const JSON_SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;

pub(super) fn deserialize_json_safe_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value <= JSON_SAFE_INTEGER_MAX {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "integer exceeds JavaScript safe integer maximum",
        ))
    }
}

pub(super) fn deserialize_optional_json_safe_u64<'de, D>(
    deserializer: D,
) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<u64>::deserialize(deserializer)? {
        Some(value) if value > JSON_SAFE_INTEGER_MAX => Err(serde::de::Error::custom(
            "integer exceeds JavaScript safe integer maximum",
        )),
        value => Ok(value),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StorageExhaustionError {
    #[error("{value} exceeds the maximum JSON-safe {kind} value")]
    OutOfRange { kind: &'static str, value: u64 },
    #[error("{kind} storage is exhausted")]
    Exhausted { kind: &'static str },
}

macro_rules! checked_counter {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            pub fn try_new(value: u64) -> Result<Self, StorageExhaustionError> {
                if value <= JSON_SAFE_INTEGER_MAX {
                    Ok(Self(value))
                } else {
                    Err(StorageExhaustionError::OutOfRange {
                        kind: $kind,
                        value,
                    })
                }
            }

            pub fn get(self) -> u64 {
                self.0
            }

            pub fn checked_next(self) -> Result<Self, StorageExhaustionError> {
                if self.0 == JSON_SAFE_INTEGER_MAX {
                    Err(StorageExhaustionError::Exhausted { kind: $kind })
                } else {
                    Ok(Self(self.0 + 1))
                }
            }
        }

        impl TryFrom<u64> for $name {
            type Error = StorageExhaustionError;

            fn try_from(value: u64) -> Result<Self, Self::Error> {
                Self::try_new(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_u64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::try_new(u64::deserialize(deserializer)?)
                    .map_err(serde::de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "integer",
                    "minimum": 0,
                    "maximum": JSON_SAFE_INTEGER_MAX
                })
            }
        }
    };
}

checked_counter!(Cursor, "cursor");
checked_counter!(TaskRevision, "task revision");

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RevisionToken(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("revision token must contain exactly 64 lowercase hexadecimal characters")]
pub struct RevisionTokenError;

impl RevisionToken {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, RevisionTokenError> {
        let value = value.as_ref();
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(RevisionTokenError)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RevisionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for RevisionToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RevisionToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for RevisionToken {
    fn schema_name() -> Cow<'static, str> {
        "RevisionToken".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[0-9a-f]{64}$"
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Draft,
    Queued,
    Running,
    NeedsAttention,
    Stopping,
    Completed,
    Stopped,
    Failed,
    Interrupted,
}

impl TaskState {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Running | Self::NeedsAttention | Self::Stopping
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Running,
    NeedsAttention,
    Stopping,
    Completed,
    Stopped,
    Failed,
    Interrupted,
}

impl RunState {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Running | Self::NeedsAttention | Self::Stopping
        )
    }
}
