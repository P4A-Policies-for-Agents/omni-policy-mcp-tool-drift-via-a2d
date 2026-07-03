# Roadmap — MCP Tool Drift Detection (via A²D)

## Now (v0.1)

- GCL schema with three decision sources (`cache` / `remote-pdp` /
  `hybrid`) and three modes (`enforce` / `observe` / `warn`).
- `SpecCache` canonical-hash diff: unchanged / descriptor-drift /
  unpinned classification.
- Dual request/response filter entrypoint that emits evidence and (in
  `enforce`) strips drifted tools from `tools/list`, on both the
  `application/json` and `text/event-stream` transports.
- **Real outbound A²D client** (`HttpClient` + `Service`): `fetch_spec`,
  `validate`, and `report`, with lazy spec load memoized in
  `PolicyState` and refreshed on `refreshIntervalSec`.
- **Managed-gateway loopback mode** (`a2d.pinPathPrefix`) — dispatch to
  the gateway's internal listener through the shared `/a2d-pin` route to
  sidestep egress-`Host` mangling.
- Deterministic FNV-1a sampler driving the `hybrid` PDP audit.
- `A2dRef` URL construction for spec / validate / evidence, with the
  loopback path prefix.

## Short-term (v0.2)

- **Wire the PDP verdict into enforcement** for `remote-pdp` — today
  `validate()` is a best-effort audit alongside the local spec diff;
  make the PDP verdict authoritative on the hot path with a circuit
  breaker and `failOpen.onPdpUnavailable` fallback to cache.
- **`pdp_disagreement` emission** — compare the sampled PDP verdict to
  the local verdict in `hybrid` and emit the divergence event.
- **Cross-replica spec cache** — thread the `CacheBuilder` handle so
  the spec is shared across gateway replicas instead of per-instance.
- **Evidence POST** — call `report()` from the emit path (behind
  `evidence.reportToA2d`) so events reach A²D, not just local logs.
- **Asset-version surfacing** from the PDP response in `remote-pdp`
  (today the version is the cached spec's).

## Medium-term (v0.3)

- **Streaming `tools/list`** support (some MCP servers emit SSE chunks
  for large tool sets).
- **Circuit breaker** in front of `remote-pdp` so a flapping PDP
  trips into cache mode automatically.
- **Schema-level diff** — surface which field of `inputSchema`
  changed (today the diff is hash-level).
- **Cross-policy correlation header** — emit
  `X-A2D-Correlation-Id` so the Govern policy and tool-poisoning
  policies share a request id.

## Long-term (v1.0)

- **Differential rollout** — read a canary flag from the spec and
  apply drift verdicts only to a fraction of traffic.
- **Replay against A²D Test Lab** — emit a "drift detected" replay
  bundle that re-runs in A²D so a reviewer can trigger a re-test
  immediately from the evidence event.
- **Multi-tenant API key rotation** — A²D pushes a rotation hint to
  the policy via the spec response.

## Risk register

| Risk | Mitigation |
|---|---|
| PDP becomes a hot dependency on the request path. | `pdpTimeoutMs` defaults to 250 ms; `failOpen.onPdpUnavailable=true` defaults to falling back to cache; `hybrid` mode keeps the hot path on cache. |
| Stale cache silently allows drift. | `refreshIntervalSec` defaults to 5 min; cache-staleness watermark surfaces in `spec_stale` evidence; `hybrid` audits a sampled fraction against real-time PDP. |
| Hybrid sampler hot-spotting a flaky tool. | Sampler is deterministic per request — a request lands on the same yes/no consistently, no thundering herd. |
| Spec unavailable at cold start blocks all MCP traffic. | `failOpen.onSpecUnavailable` is opt-in; default is closed (block + evidence) so a misconfigured asset is loud, not silent. |
| PDP disagrees with cache but cache wins. | `pdp_disagreement` evidence event fires for every divergence; A²D dashboards surface them so the cache invariably catches up. |
