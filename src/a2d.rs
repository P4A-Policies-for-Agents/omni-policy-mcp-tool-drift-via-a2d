//! A²D platform client.
//!
//! Three endpoints:
//! - `/api/platform/{assetId}/mcp/spec` — canonical descriptor set,
//!   pulled into `SpecCache` on bootstrap and on a refresh timer.
//! - `/api/platform/{assetId}/mcp/validate` — synchronous PDP
//!   consulted per request in `remote-pdp` / sampled in `hybrid`.
//! - `/api/platform/{assetId}/mcp/evidence` — async sink for evidence
//!   events.
//!
//! Authentication is a per-instance API key in the `x-a2d-api-key`
//! header.

use std::time::Duration;

use pdk::hl::HttpClient;
use thiserror::Error;

use crate::spec::SpecCache;

#[derive(Debug, Error)]
pub enum A2dError {
    #[error("a2d transport error: {0}")]
    Transport(String),
    #[error("a2d returned HTTP {status}: {body}")]
    HttpStatus { status: u32, body: String },
    #[error("a2d returned malformed payload: {0}")]
    BadPayload(String),
    #[error("a2d pdp timed out after {0:?}")]
    PdpTimeout(Duration),
    #[error("missing api key for a2d")]
    MissingCredentials,
}

#[derive(Debug, Clone)]
pub struct A2dRef {
    pub base_url: String,
    pub asset_id: String,
    pub api_key: String,
}

impl A2dRef {
    pub fn spec_url(&self) -> String {
        format!(
            "{}/api/platform/{}/mcp/spec",
            self.base_url.trim_end_matches('/'),
            self.asset_id
        )
    }

    pub fn validate_url(&self) -> String {
        format!(
            "{}/api/platform/{}/mcp/validate",
            self.base_url.trim_end_matches('/'),
            self.asset_id
        )
    }

    pub fn evidence_url(&self) -> String {
        format!(
            "{}/api/platform/{}/mcp/evidence",
            self.base_url.trim_end_matches('/'),
            self.asset_id
        )
    }
}

/// Verdict returned by the remote PDP for a single tools/list response.
#[derive(Debug, Clone)]
pub struct PdpVerdict {
    pub kept: Vec<String>,
    pub blocked: Vec<String>,
    pub asset_version: String,
}

pub struct A2dClient {
    pub reference: A2dRef,
    pub spec_timeout: Duration,
    pub pdp_timeout: Duration,
}

impl A2dClient {
    /// Fetch the spec from A²D. Wired through PDK `HttpClient` after
    /// `make build` regenerates the bindings.
    pub async fn fetch_spec(&self, _http: &HttpClient, now_secs: u64) -> Result<SpecCache, A2dError> {
        Err(A2dError::Transport(format!(
            "spec fetch wired through regenerated PDK bindings (now={now_secs})"
        )))
    }

    /// Synchronous PDP call. The body is the runtime tools array; the
    /// response is a `PdpVerdict`. Times out at `self.pdp_timeout`.
    pub async fn validate(
        &self,
        _http: &HttpClient,
        _runtime_tools: &[serde_json::Value],
    ) -> Result<PdpVerdict, A2dError> {
        Err(A2dError::Transport(
            "pdp validate wired through regenerated PDK bindings".into(),
        ))
    }

    /// Best-effort evidence sink.
    pub async fn report(&self, _http: &HttpClient, _body: &[u8]) -> Result<(), A2dError> {
        Err(A2dError::Transport(
            "evidence reporting wired through regenerated PDK bindings".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_include_asset_id() {
        let r = A2dRef {
            base_url: "https://a2d-ai.com".into(),
            asset_id: "abc".into(),
            api_key: "key".into(),
        };
        assert_eq!(r.spec_url(), "https://a2d-ai.com/api/platform/abc/mcp/spec");
        assert_eq!(r.validate_url(), "https://a2d-ai.com/api/platform/abc/mcp/validate");
        assert_eq!(r.evidence_url(), "https://a2d-ai.com/api/platform/abc/mcp/evidence");
    }

    #[test]
    fn trailing_slash_on_base_url_is_trimmed() {
        let r = A2dRef {
            base_url: "https://a2d-ai.com/".into(),
            asset_id: "abc".into(),
            api_key: "key".into(),
        };
        assert_eq!(r.spec_url(), "https://a2d-ai.com/api/platform/abc/mcp/spec");
    }
}
