//! Deterministic per-request sampler used by `hybrid` mode.
//!
//! FNV-1a 64-bit over a correlation string; a request lands on the
//! same yes/no decision when repeated, so a flaky tool doesn't ping
//! the PDP repeatedly and a green tool keeps its decision stable.

pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001B3;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// Returns true when the correlation should fire at `rate` ∈ [0, 1].
pub fn sample_should_fire(rate: f64, correlation: &str) -> bool {
    if rate <= 0.0 {
        return false;
    }
    if rate >= 1.0 {
        return true;
    }
    let h = fnv1a_64(correlation.as_bytes());
    // Map to [0, 1) by taking the top 53 bits as a fraction.
    let frac = ((h >> 11) as f64) / ((1u64 << 53) as f64);
    frac < rate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_zero_never_fires() {
        assert!(!sample_should_fire(0.0, "anything"));
    }

    #[test]
    fn rate_one_always_fires() {
        assert!(sample_should_fire(1.0, "anything"));
    }

    #[test]
    fn same_correlation_yields_same_decision() {
        let a = sample_should_fire(0.5, "request-7");
        let b = sample_should_fire(0.5, "request-7");
        assert_eq!(a, b);
    }
}
