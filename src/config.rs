//! Typed view over the policy configuration.

use crate::generated::config::Config;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("a2d.{0} is required and must be non-empty")]
    MissingField(&'static str),
    #[error("unknown mode: {0}")]
    UnknownMode(String),
    #[error("unknown decision source: {0}")]
    UnknownDecisionSource(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Enforce,
    Observe,
    Warn,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        match s {
            "enforce" => Ok(Self::Enforce),
            "observe" => Ok(Self::Observe),
            "warn" => Ok(Self::Warn),
            other => Err(ConfigError::UnknownMode(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionSource {
    Cache,
    RemotePdp,
    Hybrid,
}

impl DecisionSource {
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        match s {
            "cache" => Ok(Self::Cache),
            "remote-pdp" | "remote_pdp" => Ok(Self::RemotePdp),
            "hybrid" => Ok(Self::Hybrid),
            other => Err(ConfigError::UnknownDecisionSource(other.to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct A2dRef {
    pub base_url: String,
    pub asset_id: String,
    pub api_key_secret_ref: String,
    pub refresh_interval_secs: u32,
    pub pdp_timeout_ms: u32,
}

#[derive(Debug, Clone)]
pub struct EnforceConfig {
    pub exact_match: bool,
    pub allow_added_tools: bool,
    pub allow_removed_tools: bool,
}

#[derive(Debug, Clone)]
pub struct DecisionConfig {
    pub source: DecisionSource,
    pub hybrid_sample_rate: f64,
}

#[derive(Debug, Clone)]
pub struct EvidenceConfig {
    pub report_to_a2d: bool,
    pub log_locally: bool,
}

#[derive(Debug, Clone)]
pub struct FailOpenConfig {
    pub on_spec_unavailable: bool,
    pub on_pdp_unavailable: bool,
}

#[derive(Debug, Clone)]
pub struct PolicyConfig {
    pub a2d: A2dRef,
    pub decision: DecisionConfig,
    pub enforce: EnforceConfig,
    pub evidence: EvidenceConfig,
    pub mode: Mode,
    pub fail_open: FailOpenConfig,
}

impl PolicyConfig {
    pub fn from_config(raw: &Config) -> Result<Self, ConfigError> {
        let v = serde_json::to_value(raw).expect("Config -> Value");
        let a2d = parse_a2d(&v)?;
        let decision = parse_decision(&v)?;
        let enforce = parse_enforce(&v);
        let evidence = parse_evidence(&v);
        let mode = Mode::parse(v.get("mode").and_then(|x| x.as_str()).unwrap_or("enforce"))?;
        let fail_open = parse_fail_open(&v);
        Ok(Self { a2d, decision, enforce, evidence, mode, fail_open })
    }
}

fn parse_a2d(v: &serde_json::Value) -> Result<A2dRef, ConfigError> {
    let e = v.get("a2d").ok_or(ConfigError::MissingField("a2d"))?;
    Ok(A2dRef {
        base_url: e
            .get("baseUrl")
            .and_then(|x| x.as_str())
            .unwrap_or("https://a2d-ai.com")
            .to_string(),
        asset_id: required_string(e, "assetId")?,
        api_key_secret_ref: required_string(e, "apiKeySecretRef")?,
        refresh_interval_secs: e
            .get("refreshIntervalSec")
            .and_then(|x| x.as_i64())
            .unwrap_or(300)
            .clamp(30, 86_400) as u32,
        pdp_timeout_ms: e
            .get("pdpTimeoutMs")
            .and_then(|x| x.as_i64())
            .unwrap_or(250)
            .clamp(25, 5_000) as u32,
    })
}

fn parse_decision(v: &serde_json::Value) -> Result<DecisionConfig, ConfigError> {
    let d = v.get("decision");
    let source =
        DecisionSource::parse(d.and_then(|x| x.get("source")).and_then(|x| x.as_str()).unwrap_or("cache"))?;
    let hybrid_sample_rate = d
        .and_then(|x| x.get("hybridSampleRate"))
        .and_then(|x| x.as_f64())
        .unwrap_or(0.1)
        .clamp(0.0, 1.0);
    Ok(DecisionConfig { source, hybrid_sample_rate })
}

fn parse_enforce(v: &serde_json::Value) -> EnforceConfig {
    let e = v.get("enforce");
    EnforceConfig {
        exact_match: e
            .and_then(|x| x.get("exactMatch"))
            .and_then(|x| x.as_bool())
            .unwrap_or(true),
        allow_added_tools: e
            .and_then(|x| x.get("allowAddedTools"))
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        allow_removed_tools: e
            .and_then(|x| x.get("allowRemovedTools"))
            .and_then(|x| x.as_bool())
            .unwrap_or(true),
    }
}

fn parse_evidence(v: &serde_json::Value) -> EvidenceConfig {
    let e = v.get("evidence");
    EvidenceConfig {
        report_to_a2d: e
            .and_then(|x| x.get("reportToA2d"))
            .and_then(|x| x.as_bool())
            .unwrap_or(true),
        log_locally: e
            .and_then(|x| x.get("logLocally"))
            .and_then(|x| x.as_bool())
            .unwrap_or(true),
    }
}

fn parse_fail_open(v: &serde_json::Value) -> FailOpenConfig {
    let f = v.get("failOpen");
    FailOpenConfig {
        on_spec_unavailable: f
            .and_then(|x| x.get("onSpecUnavailable"))
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        on_pdp_unavailable: f
            .and_then(|x| x.get("onPdpUnavailable"))
            .and_then(|x| x.as_bool())
            .unwrap_or(true),
    }
}

fn required_string(v: &serde_json::Value, field: &'static str) -> Result<String, ConfigError> {
    v.get(field)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or(ConfigError::MissingField(field))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_source_parses_known_values() {
        assert_eq!(DecisionSource::parse("cache").unwrap(), DecisionSource::Cache);
        assert_eq!(DecisionSource::parse("remote-pdp").unwrap(), DecisionSource::RemotePdp);
        assert_eq!(DecisionSource::parse("hybrid").unwrap(), DecisionSource::Hybrid);
        assert!(DecisionSource::parse("nope").is_err());
    }

    #[test]
    fn mode_parses_known_values() {
        assert_eq!(Mode::parse("enforce").unwrap(), Mode::Enforce);
        assert!(Mode::parse("yolo").is_err());
    }
}
