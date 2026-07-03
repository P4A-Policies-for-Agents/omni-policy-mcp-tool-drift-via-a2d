//! A²D platform client.
//!
//! Three endpoints:
//! - `/api/platform/{assetId}/mcp/spec` — canonical descriptor set,
//!   pulled into `SpecCache` on the first request and on a refresh
//!   timer.
//! - `/api/platform/{assetId}/mcp/validate` — synchronous PDP
//!   consulted per request in `remote-pdp` / sampled in `hybrid`.
//! - `/api/platform/{assetId}/mcp/evidence` — async sink for evidence
//!   events.
//!
//! Authentication is a per-instance A²D policy-scoped API key sent as
//! `Authorization: Bearer <key>`. Policy-scoped keys can reach only
//! `/mcp/spec`, `/mcp/validate`, `/mcp/evidence` on the A²D side —
//! see docs `features/platform-mcp/policy-keys.mdx`.
//!
//! Every request is dispatched through a `pdk::hl::Service` handle
//! (registered from the `format: service` `baseUrl` at boot) so the
//! outbound call re-uses the gateway's own Envoy upstream. The
//! `path_prefix` field supports the managed-gateway loopback route —
//! see `docs/managed-omni-gateway-setup.md`.

use std::time::Duration;

use pdk::hl::{HttpClient, Service};
use pdk::logger;
use thiserror::Error;

use crate::spec::SpecCache;

const FETCH_TIMEOUT_SECS: u64 = 30;

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

/// Addressing + credentials for a single A²D asset. Owned by the
/// caller; `A2dClient` borrows it per-request.
#[derive(Debug, Clone)]
pub struct A2dRef {
    pub base_url: String,
    pub asset_id: String,
    pub api_key: String,
    /// Prepended to every request path (e.g. `/a2d-pin`). Empty for a
    /// direct A²D call; set when routing through a gateway loopback.
    pub path_prefix: String,
}

impl A2dRef {
    pub fn spec_path(&self) -> String {
        format!("{}/api/platform/{}/mcp/spec", self.path_prefix, self.asset_id)
    }

    pub fn validate_path(&self) -> String {
        format!("{}/api/platform/{}/mcp/validate", self.path_prefix, self.asset_id)
    }

    pub fn evidence_path(&self) -> String {
        format!("{}/api/platform/{}/mcp/evidence", self.path_prefix, self.asset_id)
    }

    pub fn spec_url(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), self.spec_path())
    }

    pub fn validate_url(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), self.validate_path())
    }

    pub fn evidence_url(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), self.evidence_path())
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
    pub fn new(reference: A2dRef, pdp_timeout: Duration) -> Self {
        Self {
            reference,
            spec_timeout: Duration::from_secs(FETCH_TIMEOUT_SECS),
            pdp_timeout,
        }
    }

    /// Fetch the canonical spec from A²D and parse it into a
    /// `SpecCache`. Errors if credentials are missing, the network call
    /// fails, the status is non-2xx, or the body isn't a recognized
    /// spec payload.
    pub async fn fetch_spec(
        &self,
        http: &HttpClient,
        service: &Service,
        now_secs: u64,
    ) -> Result<SpecCache, A2dError> {
        if self.reference.api_key.is_empty() {
            return Err(A2dError::MissingCredentials);
        }

        let bearer = format!("Bearer {}", self.reference.api_key);
        let path = self.reference.spec_path();
        let authority = service.uri().authority().to_string();

        let headers = vec![
            ("host", authority.as_str()),
            ("accept", "application/json"),
            ("authorization", bearer.as_str()),
        ];

        let response = http
            .request(service)
            .path(path.as_str())
            .timeout(self.spec_timeout)
            .headers(headers)
            .get()
            .await
            .map_err(|e| A2dError::Transport(format!("{:?}", e)))?;

        let status = response.status_code();
        if !(200..300).contains(&status) {
            return Err(A2dError::HttpStatus {
                status,
                body: String::from_utf8_lossy(response.body()).to_string(),
            });
        }

        parse_spec(response.body(), now_secs)
    }

    /// Synchronous PDP call. The body is the runtime tools array; the
    /// response is a `PdpVerdict`. Dispatched with `self.pdp_timeout`.
    pub async fn validate(
        &self,
        http: &HttpClient,
        service: &Service,
        runtime_tools: &[serde_json::Value],
    ) -> Result<PdpVerdict, A2dError> {
        if self.reference.api_key.is_empty() {
            return Err(A2dError::MissingCredentials);
        }

        let bearer = format!("Bearer {}", self.reference.api_key);
        let path = self.reference.validate_path();
        let authority = service.uri().authority().to_string();
        let body = serde_json::to_vec(&serde_json::json!({ "tools": runtime_tools }))
            .map_err(|e| A2dError::BadPayload(format!("encode validate request: {e}")))?;

        let headers = vec![
            ("host", authority.as_str()),
            ("accept", "application/json"),
            ("content-type", "application/json"),
            ("authorization", bearer.as_str()),
        ];

        let response = http
            .request(service)
            .path(path.as_str())
            .timeout(self.pdp_timeout)
            .headers(headers)
            .body(&body)
            .post()
            .await
            .map_err(|e| A2dError::Transport(format!("{:?}", e)))?;

        let status = response.status_code();
        if !(200..300).contains(&status) {
            return Err(A2dError::HttpStatus {
                status,
                body: String::from_utf8_lossy(response.body()).to_string(),
            });
        }

        parse_verdict(response.body())
    }

    /// Best-effort evidence sink. Errors are surfaced but the caller
    /// should log-and-drop; evidence reporting is not on the critical
    /// path.
    pub async fn report(
        &self,
        http: &HttpClient,
        service: &Service,
        body: &[u8],
    ) -> Result<(), A2dError> {
        if self.reference.api_key.is_empty() {
            return Err(A2dError::MissingCredentials);
        }

        let bearer = format!("Bearer {}", self.reference.api_key);
        let path = self.reference.evidence_path();
        let authority = service.uri().authority().to_string();

        let headers = vec![
            ("host", authority.as_str()),
            ("accept", "application/json"),
            ("content-type", "application/json"),
            ("authorization", bearer.as_str()),
        ];

        let response = http
            .request(service)
            .path(path.as_str())
            .timeout(self.spec_timeout)
            .headers(headers)
            .body(body)
            .post()
            .await
            .map_err(|e| A2dError::Transport(format!("{:?}", e)))?;

        let status = response.status_code();
        if !(200..300).contains(&status) {
            logger::debug!("a2d evidence POST returned HTTP {} — dropping", status);
            return Err(A2dError::HttpStatus {
                status,
                body: String::from_utf8_lossy(response.body()).to_string(),
            });
        }
        Ok(())
    }
}

/// Parse the A²D `/mcp/spec` response body into a `SpecCache`.
///
/// Three shapes are accepted so the policy can pin against either the
/// A²D live-wire response or a hand-authored fixture:
///
/// - `{ "assetVersion": "...", "payload": { "tools": [<descriptor>...] } }` — live A²D shape
/// - `{ "assetVersion": "...", "tools": [<descriptor>...] }`
/// - `{ "tools": [<descriptor>...] }`
pub fn parse_spec(body: &[u8], now_secs: u64) -> Result<SpecCache, A2dError> {
    let v: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| A2dError::BadPayload(format!("json decode: {e}")))?;
    let asset_version = v
        .get("assetVersion")
        .and_then(|s| s.as_str())
        .unwrap_or("unknown")
        .to_string();
    // Live A²D wraps tools under `payload.tools`; fixtures put them at top level.
    let tools = v
        .get("payload")
        .and_then(|p| p.get("tools"))
        .and_then(|t| t.as_array())
        .or_else(|| v.get("tools").and_then(|t| t.as_array()))
        .ok_or_else(|| A2dError::BadPayload("missing tools[] array".into()))?;
    Ok(SpecCache::from_descriptors(&asset_version, now_secs, tools))
}

/// Parse the A²D `/mcp/validate` PDP response into a `PdpVerdict`.
/// Accepts the same top-level / `payload`-wrapped shapes as `parse_spec`.
pub fn parse_verdict(body: &[u8]) -> Result<PdpVerdict, A2dError> {
    let v: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| A2dError::BadPayload(format!("json decode: {e}")))?;
    let asset_version = v
        .get("assetVersion")
        .and_then(|s| s.as_str())
        .unwrap_or("unknown")
        .to_string();
    let names = |key: &str| -> Vec<String> {
        v.get("payload")
            .and_then(|p| p.get(key))
            .and_then(|x| x.as_array())
            .or_else(|| v.get(key).and_then(|x| x.as_array()))
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(PdpVerdict {
        kept: names("kept"),
        blocked: names("blocked"),
        asset_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ref(base: &str, prefix: &str) -> A2dRef {
        A2dRef {
            base_url: base.into(),
            asset_id: "abc".into(),
            api_key: "key".into(),
            path_prefix: prefix.into(),
        }
    }

    #[test]
    fn urls_include_asset_id() {
        let r = make_ref("https://a2d-ai.com", "");
        assert_eq!(r.spec_url(), "https://a2d-ai.com/api/platform/abc/mcp/spec");
        assert_eq!(
            r.validate_url(),
            "https://a2d-ai.com/api/platform/abc/mcp/validate"
        );
        assert_eq!(
            r.evidence_url(),
            "https://a2d-ai.com/api/platform/abc/mcp/evidence"
        );
    }

    #[test]
    fn trailing_slash_on_base_url_is_trimmed() {
        let r = make_ref("https://a2d-ai.com/", "");
        assert_eq!(r.spec_url(), "https://a2d-ai.com/api/platform/abc/mcp/spec");
    }

    #[test]
    fn path_prefix_is_prepended_to_paths() {
        let r = make_ref("http://127.0.0.1:8081", "/a2d-pin");
        assert_eq!(r.spec_path(), "/a2d-pin/api/platform/abc/mcp/spec");
        assert_eq!(r.validate_path(), "/a2d-pin/api/platform/abc/mcp/validate");
        assert_eq!(r.evidence_path(), "/a2d-pin/api/platform/abc/mcp/evidence");
        assert_eq!(
            r.spec_url(),
            "http://127.0.0.1:8081/a2d-pin/api/platform/abc/mcp/spec"
        );
    }

    #[test]
    fn empty_prefix_yields_bare_paths() {
        let r = make_ref("https://a2d-ai.com", "");
        assert_eq!(r.spec_path(), "/api/platform/abc/mcp/spec");
    }

    #[test]
    fn parse_spec_accepts_live_payload_wrapper() {
        let body = br#"{"assetVersion":"2026-06-29","specHash":"sha256:abc","payload":{"tools":[{"name":"t_wrapped","description":"d"}]}}"#;
        let spec = parse_spec(body, 7).expect("valid live body");
        assert_eq!(spec.asset_version, "2026-06-29");
        assert!(spec.tools.contains_key("t_wrapped"));
    }

    #[test]
    fn parse_spec_accepts_versioned_body() {
        let body = br#"{"assetVersion":"1.2.3","tools":[{"name":"t1","description":"d"}]}"#;
        let spec = parse_spec(body, 42).expect("valid body");
        assert_eq!(spec.asset_version, "1.2.3");
        assert_eq!(spec.fetched_at_epoch_secs, 42);
        assert!(spec.tools.contains_key("t1"));
    }

    #[test]
    fn parse_spec_defaults_asset_version() {
        let body = br#"{"tools":[{"name":"t2","description":"d"}]}"#;
        let spec = parse_spec(body, 0).expect("valid body");
        assert_eq!(spec.asset_version, "unknown");
        assert!(spec.tools.contains_key("t2"));
    }

    #[test]
    fn parse_spec_rejects_missing_tools() {
        let body = br#"{"assetVersion":"1"}"#;
        assert!(matches!(parse_spec(body, 0), Err(A2dError::BadPayload(_))));
    }

    #[test]
    fn parse_spec_rejects_bad_json() {
        let body = br#"not json"#;
        assert!(matches!(parse_spec(body, 0), Err(A2dError::BadPayload(_))));
    }

    #[test]
    fn parse_verdict_reads_kept_and_blocked() {
        let body = br#"{"assetVersion":"9","kept":["a","b"],"blocked":["c"]}"#;
        let verdict = parse_verdict(body).expect("valid verdict");
        assert_eq!(verdict.asset_version, "9");
        assert_eq!(verdict.kept, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(verdict.blocked, vec!["c".to_string()]);
    }

    #[test]
    fn parse_verdict_accepts_payload_wrapper() {
        let body = br#"{"payload":{"kept":["x"],"blocked":[]}}"#;
        let verdict = parse_verdict(body).expect("valid verdict");
        assert_eq!(verdict.kept, vec!["x".to_string()]);
        assert!(verdict.blocked.is_empty());
    }
}
