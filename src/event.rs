use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const SCHEMA_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Inbound,
    Outbound,
    Neutral,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NormalizedEvent {
    pub v: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub magnitude: f64,
    #[serde(default)]
    pub direction: Direction,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl NormalizedEvent {
    pub fn validate(self) -> Result<Self, EventError> {
        if self.v != SCHEMA_VERSION {
            return Err(EventError::UnsupportedVersion(self.v));
        }
        if !self.magnitude.is_finite() || self.magnitude < 0.0 {
            return Err(EventError::InvalidMagnitude);
        }
        if self.timestamp.is_some_and(|value| !value.is_finite()) {
            return Err(EventError::InvalidTimestamp);
        }
        Ok(self)
    }

    pub fn flow_key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}",
            self.origin.as_deref().unwrap_or(""),
            self.target.as_deref().unwrap_or(""),
            self.category
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("unsupported schema version {0}; supported version is 1")]
    UnsupportedVersion(u8),
    #[error("magnitude must be finite and non-negative")]
    InvalidMagnitude,
    #[error("timestamp must be finite")]
    InvalidTimestamp,
}

pub fn schema_document() -> Result<String> {
    Ok(serde_json::to_string_pretty(&json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://synesthesia.invalid/schema/event-v1.json",
        "title": "Synesthesia normalized event v1",
        "type": "object",
        "required": ["v", "category", "magnitude"],
        "properties": {
            "v": {"const": 1},
            "timestamp": {"type": ["number", "null"], "description": "Source-defined seconds; fractions allowed. Producers must document their clock domain."},
            "category": {"type": "string"},
            "origin": {"type": ["string", "null"]},
            "target": {"type": ["string", "null"]},
            "magnitude": {"type": "number", "minimum": 0},
            "direction": {"enum": ["inbound", "outbound", "neutral", "unknown"], "default": "unknown"},
            "labels": {"type": "object", "additionalProperties": {"type": "string"}}
        },
        "additionalProperties": false,
        "example": {
            "v": 1,
            "timestamp": 1720000000.125,
            "category": "tcp",
            "origin": "10.0.0.4:54321",
            "target": "10.0.0.8:443",
            "magnitude": 1514,
            "direction": "outbound",
            "labels": {"protocol": "TLS"}
        }
    }))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_v1_shape() {
        let input = r#"{"v":1,"timestamp":1720000000.125,"category":"tcp","origin":"10.0.0.4:54321","target":"10.0.0.8:443","magnitude":1514,"direction":"outbound","labels":{"protocol":"TLS"}}"#;
        let event = serde_json::from_str::<NormalizedEvent>(input)
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(event.category, "tcp");
        assert_eq!(event.direction, Direction::Outbound);
        assert_eq!(event.labels["protocol"], "TLS");
    }

    #[test]
    fn optional_fields_degrade_gracefully() {
        let event =
            serde_json::from_str::<NormalizedEvent>(r#"{"v":1,"category":"pulse","magnitude":2}"#)
                .unwrap()
                .validate()
                .unwrap();
        assert_eq!(event.direction, Direction::Unknown);
        assert!(event.origin.is_none());
        assert!(event.labels.is_empty());
    }

    #[test]
    fn refuses_unsupported_versions_explicitly() {
        let event =
            serde_json::from_str::<NormalizedEvent>(r#"{"v":9,"category":"future","magnitude":1}"#)
                .unwrap();
        assert!(matches!(
            event.validate(),
            Err(EventError::UnsupportedVersion(9))
        ));
    }
}
