//! Spec cache module (stub).
//!
//! The background-refresh design needs PDK injections that are not yet
//! wired into the entrypoint (HttpClient via the filter closure, plus a
//! TTL strategy on `CacheBuilder`). Until that lands, callers should
//! treat the cached spec as `None` and rely on the response filter's
//! `SpecUnavailable` evidence path.

use crate::spec::SpecCache;

#[derive(Clone, Debug)]
pub struct CachedSpec {
    pub spec: SpecCache,
    pub fetched_at: u64,
    pub expires_at: u64,
}

impl CachedSpec {
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }
}
