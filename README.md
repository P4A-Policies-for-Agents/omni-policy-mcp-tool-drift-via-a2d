# MCP Tool Drift Detection (via A²D)

A Mulesoft Flex / Omni Gateway custom policy that detects MCP **tool
drift** — runtime `tools/list` responses that have diverged from the
spec approved in A²D — and (optionally) strips the drifted tools from
the response before it reaches the LLM client.

The interesting axis of this policy is **where the decision comes
from**.

---

## Decision sources

| Source | What it does | Latency cost | Freshness |
|---|---|---|---|
| `cache` | Decides locally from a refreshed spec cache (LKG). | ~0 ms | Bounded by `refreshIntervalSec` (default 5 min). |
| `remote-pdp` | Calls A²D's PDP per request (`/api/platform/{assetId}/mcp/validate`). | One round-trip; capped at `pdpTimeoutMs` (default 250 ms). | Real-time. |
| `hybrid` | Decides locally, **also** calls the PDP asynchronously for a sampled fraction of requests. Divergence raises `pdp_disagreement` evidence. | ~0 ms on the hot path. | LKG on the hot path; sampled real-time audit. |

`hybrid` is the recommended default: latency of `cache` with the
auditability of `remote-pdp`. The sample rate is deterministic per
request via FNV-1a so a hot tool doesn't repeatedly tax the PDP.

---

## Decision modes

Orthogonal to source — *what to do once the verdict is known*.

- `enforce` — strip the drifted tool from the response.
- `warn` — pass through with `x-mcp-drift-warning` evidence.
- `observe` — emit evidence only.

---

## Configuration

| Path | Type | Default | Notes |
|---|---|---|---|
| `a2d.baseUrl` | string | `https://a2d-ai.com` | |
| `a2d.assetId` | string | required | A²D MCP asset id. |
| `a2d.apiKeySecretRef` | string | required | Per-instance API key. |
| `a2d.refreshIntervalSec` | int 30–86400 | 300 | Cache mode spec refresh. |
| `a2d.pdpTimeoutMs` | int 25–5000 | 250 | Per-request PDP timeout. |
| `decision.source` | enum | `cache` | `cache` / `remote-pdp` / `hybrid`. |
| `decision.hybridSampleRate` | float 0–1 | `0.1` | Hybrid PDP audit rate. |
| `enforce.exactMatch` | bool | `true` | Strict hash equality. |
| `enforce.allowAddedTools` | bool | `false` | |
| `enforce.allowRemovedTools` | bool | `true` | |
| `evidence.reportToA2d` | bool | `true` | POST every event to A²D. |
| `evidence.logLocally` | bool | `true` | Emit JSON log lines. |
| `mode` | enum | `enforce` | `enforce` / `observe` / `warn`. |
| `failOpen.onSpecUnavailable` | bool | `false` | Allow traffic when cache is empty. |
| `failOpen.onPdpUnavailable` | bool | `true` | Fall back to cache when PDP is down. |

---

## Evidence

Every decision lands as a JSON log line and (when `reportToA2d=true`)
POSTs to `{baseUrl}/api/platform/{assetId}/mcp/evidence`.

```json
{
  "class": "descriptor_drift",
  "severity": "critical",
  "decision": "stripped",
  "source": "hybrid",
  "asset_id": "demo-mcp-asset",
  "asset_version": "1.4.2",
  "tool_name": "get_user",
  "local_verdict": "descriptor_drift",
  "pdp_verdict": "descriptor_drift"
}
```

`class` ∈ `descriptor_drift | unpinned_tool | removed_tool |
spec_unavailable | spec_stale | pdp_unavailable | pdp_disagreement`.

---

## Failure modes

- **PDP slow / down (remote-pdp).** Times out at `pdpTimeoutMs`; if
  `failOpen.onPdpUnavailable=true` falls back to the LKG cache and
  emits `pdp_unavailable`. Otherwise the response is blocked.
- **Spec never loaded (cold start).** `failOpen.onSpecUnavailable`
  controls allow/block; evidence event always fires.
- **PDP disagrees with cache (hybrid).** The local verdict is acted
  on, the PDP verdict is recorded, and `pdp_disagreement` evidence
  fires for post-hoc review.

---

## Live Demo

A reference deployment of this policy is running on the
`agent-network-ingress-gw` Flex Gateway in the Anypoint Sandbox
environment (org `anypoint-cbp-1780648272`).

| Field | Value |
|---|---|
| Gateway | `agent-network-ingress-gw` (id `35755bec-3177-4d32-a8c9-c9705f5b1c0b`, gw `1.13.2`) |
| Public base URL | `https://agent-network-ingress-gw-zovwbn.jeg62f.usa-e2.cloudhub.io` |
| Proxy path | `/mcp-drift-via-a2d-demo` |
| API instance | `20999089` (Exchange asset `drift-demo-a2d-mcp/1.0.0`) |
| Upstream (a2d mock) | `https://www.a2d-ai.com/api/platform/7b26e0d0-dfcf-4c6a-8484-8c907724366d/mcp` |
| Policy version (dev) | `omni-policy-mcp-tool-drift-via-a-2-d-dev/0.1.0-20260629203620` |

The upstream is an A²D-hosted MCP mock server declaring three tools
(`lookup_account`, `search_accounts`, `get_account_balance`). It is the
source of truth for the pinned descriptor set this policy enforces.

### Try it

`tools/list` should return the three pinned tools unchanged:

```bash
curl -sS -X POST \
  https://agent-network-ingress-gw-zovwbn.jeg62f.usa-e2.cloudhub.io/mcp-drift-via-a2d-demo \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"tools/list","id":1}'
```

To exercise drift, mutate one tool's description in the A²D mock UI
(asset id `7b26e0d0-…366d`) and re-issue the request. The policy
strips the drifted tool and POSTs a `descriptor_drift` evidence event
to `https://www.a2d-ai.com/api/policy/evidence`. The matching
`policy_evidence` row appears in A²D Test Lab under "Runtime Runs."

Note: the policy config currently uses placeholder secrets
(`REPLACE_WITH_PLATFORM_API_KEY`). Swap in real Flex secret refs via
Anypoint Secrets Manager and re-apply the policy before the upstream
PDP call will authenticate.

---

## Build, test, run

```bash
make setup
make build
make test
make run
make publish
make release
```

`make build` runs `cargo anypoint config-gen` against
`definition/gcl.yaml`, which overwrites `src/generated/config.rs`.

---

## License

Copyright 2026 Salesforce, Inc. All rights reserved.
