// Copyright 2026 Salesforce, Inc. All rights reserved.

//! Deterministic FNV-1a sampling for hybrid PDP audit.

mod common;

use omni_policy_mcp_tool_drift_via_a2d::sampler::{fnv1a_64, sample_should_fire};

#[test]
fn fnv1a_is_stable() {
    assert_eq!(fnv1a_64(b"abc"), fnv1a_64(b"abc"));
    assert_ne!(fnv1a_64(b"abc"), fnv1a_64(b"abd"));
}

#[test]
fn rate_zero_never_fires() {
    for i in 0..1_000 {
        assert!(!sample_should_fire(0.0, &format!("req-{i}")));
    }
}

#[test]
fn rate_one_always_fires() {
    for i in 0..1_000 {
        assert!(sample_should_fire(1.0, &format!("req-{i}")));
    }
}

#[test]
fn rate_one_tenth_fires_roughly_one_tenth_of_the_time() {
    let n = 5_000;
    let hits: usize = (0..n)
        .filter(|i| sample_should_fire(0.1, &format!("req-{i}")))
        .count();
    let pct = hits as f64 / n as f64;
    assert!((0.07..=0.13).contains(&pct), "got {pct}");
}

#[test]
fn same_correlation_yields_same_decision() {
    let a = sample_should_fire(0.42, "request-7");
    let b = sample_should_fire(0.42, "request-7");
    assert_eq!(a, b);
}
