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
        let a2d = parse_a2d(&raw.a_2_d)?;
        let decision = parse_decision(raw.decision.as_ref())?;
        let enforce = parse_enforce(raw.enforce.as_ref());
        let evidence = parse_evidence(raw.evidence.as_ref());
        let mode = Mode::parse(raw.mode.as_deref().unwrap_or("enforce"))?;
        let fail_open = parse_fail_open(raw.fail_open.as_ref());
        Ok(Self { a2d, decision, enforce, evidence, mode, fail_open })
    }
}

fn require(value: &str, field: &'static str) -> Result<String, ConfigError> {
    if value.is_empty() {
        Err(ConfigError::MissingField(field))
    } else {
        Ok(value.to_string())
    }
}

fn parse_a2d(e: &crate::generated::config::A2DConfig) -> Result<A2dRef, ConfigError> {
    Ok(A2dRef {
        base_url: e
            .base_url
            .clone()
            .unwrap_or_else(|| "https://a2d-ai.com".to_string()),
        asset_id: require(&e.asset_id, "assetId")?,
        api_key_secret_ref: require(&e.api_key_secret_ref, "apiKeySecretRef")?,
        refresh_interval_secs: e
            .refresh_interval_sec
            .unwrap_or(300)
            .clamp(30, 86_400) as u32,
        pdp_timeout_ms: e
            .pdp_timeout_ms
            .unwrap_or(250)
            .clamp(25, 5_000) as u32,
    })
}

fn parse_decision(
    d: Option<&crate::generated::config::DecisionConfig>,
) -> Result<DecisionConfig, ConfigError> {
    let source = DecisionSource::parse(
        d.and_then(|x| x.source.as_deref()).unwrap_or("cache"),
    )?;
    let hybrid_sample_rate = d
        .and_then(|x| x.hybrid_sample_rate)
        .unwrap_or(0.1)
        .clamp(0.0, 1.0);
    Ok(DecisionConfig { source, hybrid_sample_rate })
}

fn parse_enforce(
    e: Option<&crate::generated::config::EnforceConfig>,
) -> EnforceConfig {
    EnforceConfig {
        exact_match: e.and_then(|x| x.exact_match).unwrap_or(true),
        allow_added_tools: e.and_then(|x| x.allow_added_tools).unwrap_or(false),
        allow_removed_tools: e.and_then(|x| x.allow_removed_tools).unwrap_or(true),
    }
}

fn parse_evidence(
    e: Option<&crate::generated::config::EvidenceConfig>,
) -> EvidenceConfig {
    EvidenceConfig {
        report_to_a2d: e.and_then(|x| x.report_to_a_2_d).unwrap_or(true),
        log_locally: e.and_then(|x| x.log_locally).unwrap_or(true),
    }
}

fn parse_fail_open(
    f: Option<&crate::generated::config::FailOpenConfig>,
) -> FailOpenConfig {
    FailOpenConfig {
        on_spec_unavailable: f.and_then(|x| x.on_spec_unavailable).unwrap_or(false),
        on_pdp_unavailable: f.and_then(|x| x.on_pdp_unavailable).unwrap_or(true),
    }
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
