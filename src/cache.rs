//! Spec cache module with background refresh via Timer.
//!
//! In `cache` decision mode the policy reads from a local `SpecCache` that
//! is periodically refreshed from A²D. The refresh is driven by a PDK
//! `Timer::period(...)` task that wakes every `a2d.refreshIntervalSec` and
//! fetches the latest spec. The cached value embeds an expiry timestamp so
//! consumers can detect stale data.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use pdk::cache::CacheBuilder;
use pdk::hl::{Clock, HttpClient, Timer};
use pdk::logger;

use crate::a2d::A2dClient;
use crate::spec::SpecCache;

/// Cached spec with embedded expiry.
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

/// Wrapper around `CacheBuilder` that starts a background refresh task.
pub struct SpecCacheManager {
    cache: Rc<RefCell<Option<CachedSpec>>>,
}

impl SpecCacheManager {
    /// Initialize the cache and spawn a background refresh task.
    pub fn new(
        _builder: CacheBuilder,
        timer: Timer,
        clock: Clock,
        client: HttpClient,
        a2d_client: A2dClient,
        refresh_interval: Duration,
    ) -> Self {
        let cache = Rc::new(RefCell::new(None));
        let cache_clone = cache.clone();

        // Spawn background refresh task
        pdk::hl::spawn(async move {
            let mut interval = timer.period(refresh_interval);
            loop {
                interval.tick().await;
                let now = clock.now().as_secs();

                match a2d_client.fetch_spec(&client, now).await {
                    Ok(spec) => {
                        let expires_at = now + refresh_interval.as_secs();
                        let cached = CachedSpec {
                            spec,
                            fetched_at: now,
                            expires_at,
                        };
                        *cache_clone.borrow_mut() = Some(cached);
                        logger::info!(
                            "mcp-drift-a2d: spec refreshed (asset={}, expires={})",
                            a2d_client.reference.asset_id,
                            expires_at
                        );
                    }
                    Err(e) => {
                        logger::warn!("mcp-drift-a2d: spec refresh failed: {}", e);
                    }
                }
            }
        });

        Self { cache }
    }

    /// Get the current cached spec (None if never fetched or expired).
    pub fn get(&self) -> Option<CachedSpec> {
        self.cache.borrow().clone()
    }
}
