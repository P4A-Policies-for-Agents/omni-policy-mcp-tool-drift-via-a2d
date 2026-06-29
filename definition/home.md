# MCP Tool Drift Detection (via A²D)

Detects MCP **tool drift** — a runtime `tools/list` response that
diverges from the spec approved in A²D. The pin (the canonical spec)
is the customer's A²D asset; the gateway is the enforcement point.

## Decision sources

- **`cache`** — local LKG cache of the A²D spec, refreshed on a timer.
  Lowest latency. Spec staleness is bounded by `refreshIntervalSec`.
- **`remote-pdp`** — every request asks A²D's policy-decision endpoint
  whether the runtime descriptor set is still valid. Always reflects
  the latest spec; adds one network hop to every MCP call.
- **`hybrid`** — decide locally for latency, audit a sampled fraction
  of decisions against the PDP. A disagreement raises a
  `pdp_disagreement` evidence event.

## Decision modes

`enforce` strips drifted tools from the response. `warn` annotates the
response with `x-mcp-drift-warning`. `observe` emits evidence only.

## Where decisions come from

A²D owns the spec — every emitted event includes `policy_instance_id`
and the asset version so A²D Test Lab can correlate runtime drift to
the approving identity.
