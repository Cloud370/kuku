use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use super::payload::EventPayload;

/// A single event persisted in a session's events.jsonl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    pub id: u64,
    pub payload: EventPayload,
}

impl Serialize for StoredEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.payload {
            EventPayload::Unknown(value) => value.serialize(serializer),
            payload => {
                let value = payload
                    .to_new_json(self.id)
                    .map_err(serde::ser::Error::custom)?;
                value.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for StoredEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| de::Error::custom("stored event must be a JSON object"))?;
        let id = object
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| de::Error::custom("stored event is missing numeric id"))?;

        match EventPayload::from_json_object(object) {
            Some(payload) => Ok(Self { id, payload }),
            None => Ok(Self {
                id,
                payload: EventPayload::Unknown(value),
            }),
        }
    }
}
