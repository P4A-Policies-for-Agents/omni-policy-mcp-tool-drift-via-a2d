# Roadmap — MCP Tool Drift Detection (via A²D)

## Now (v0.1)

- GCL schema with three decision sources (`cache` / `remote-pdp` /
  `hybrid`) and three modes (`enforce` / `observe` / `warn`).
- `SpecCache` canonical-hash diff: unchanged / descriptor-drift /
  unpinned classification.
- Response-filter entrypoint that emits evidence and (in `enforce`)
  strips drifted tools from `tools/list`.
- Deterministic FNV-1a sampler for hybrid PDP audit.
- `A2dRef` URL construction for spec / validate / evidence.
- Five integration test files exercising diff, decision source,
  sampler, URL construction, and config loading.

## Short-term (v0.2)

- **HttpClient wiring** for spec fetch, PDP validate, and evidence
  POST — once the cargo-anypoint codegen runs at `make build` and
  publishes the dispatch shapes.
- **Timer + Clock** for spec refresh on the configured cadence; LKG
  on transient PDP/spec failure.
- **`remote-pdp` request path** — call validate, honor `pdpTimeoutMs`,
  fall back to cache on timeout per `failOpen.onPdpUnavailable`.
- **Hybrid audit fire-and-forget** — sampled PDP call with the
  sampler; compare verdict to local and emit `pdp_disagreement`.
- **Asset-version surfacing** in evidence (today the version is the
  cached one; in `remote-pdp` it will come from the PDP response).

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
