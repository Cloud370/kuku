use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug)]
pub enum ExecutionIdError {
    Random(getrandom::Error),
    InvalidFormat { expected_prefix: &'static str },
}

impl fmt::Display for ExecutionIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Random(error) => write!(formatter, "failed to generate execution identity: {error}"),
            Self::InvalidFormat { expected_prefix } => write!(
                formatter,
                "execution identity must be {expected_prefix} followed by 24 lowercase hex characters"
            ),
        }
    }
}

impl std::error::Error for ExecutionIdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Random(error) => Some(error),
            Self::InvalidFormat { .. } => None,
        }
    }
}

fn encode_hex(bytes: &[u8; 12]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(24);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value.len() == prefix.len() + 24
        && value.starts_with(prefix)
        && value[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

macro_rules! execution_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn try_new() -> Result<Self, ExecutionIdError> {
                let mut bytes = [0_u8; 12];
                getrandom::fill(&mut bytes).map_err(ExecutionIdError::Random)?;
                Ok(Self(format!("{}{}", $prefix, encode_hex(&bytes))))
            }

            pub fn parse(value: impl AsRef<str>) -> Result<Self, ExecutionIdError> {
                let value = value.as_ref();
                if valid_id(value, $prefix) {
                    Ok(Self(value.to_owned()))
                } else {
                    Err(ExecutionIdError::InvalidFormat {
                        expected_prefix: $prefix,
                    })
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = ExecutionIdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(serde::de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "pattern": concat!("^", $prefix, "[0-9a-f]{24}$")
                })
            }
        }
    };
}

execution_id!(TaskId, "tsk_");
execution_id!(RunId, "run_");
execution_id!(TurnId, "trn_");
execution_id!(RequestId, "req_");
execution_id!(InteractionId, "int_");
execution_id!(ConversationId, "con_");
execution_id!(WorkspaceId, "wsp_");
execution_id!(ReviewSubmissionId, "rsub_");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionScope {
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub turn_id: TurnId,
    pub conversation_id: ConversationId,
    #[serde(deserialize_with = "super::task::deserialize_json_safe_u64")]
    #[schemars(range(max = super::task::JSON_SAFE_INTEGER_MAX))]
    pub turn_index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestScope {
    pub execution: ExecutionScope,
    pub request_id: RequestId,
}
