# MCP Tool Drift Detection (via A²D)

A Mulesoft Flex / Omni Gateway custom policy that detects MCP **tool
drift** — runtime `tools/list` responses that have diverged from the
spec approved in A²D — and (optionally) strips the drifted tools from
the response before it reaches the LLM client.

The interesting axis of this policy is **where the decision comes
from**.

---

## Purpose & business need

### The problem

An LLM agent calling an MCP server reads the `tools/list` response
before deciding which tool to invoke. The description, the input
schema, the output shape, the annotations — every byte of that
descriptor influences whether the agent picks the tool, what
parameters it sends, and what it does with the response.

That descriptor set is reviewed, approved, and signed off on inside
A²D as part of the asset lifecycle. After that, **anything can
happen**:

- The MCP server is redeployed with a "small" copy edit to a tool
  description.
- A new input field is added to a schema "just to capture more
  context."
- An upstream library bumps a version and quietly rewrites the tool's
  output shape.
- An attacker who reached the upstream changes a description to
  include instructions the LLM will obey.

None of those changes go back through A²D approval. The agent calling
the tool sees the new descriptor and acts on it.

### Why this policy

A²D already owns the canonical, approved descriptor set per asset
(see `generateMCPSpec()` and the platform API). This policy makes
that approval load-bearing at runtime:

- **Continuous compliance** — every `tools/list` response is hashed
  and compared to the A²D-canonical pin. Any field-level change
  surfaces as `description_changed` / `input_schema_changed` /
  `output_schema_changed` / `annotation_changed` evidence.
- **Decision locality, your call** — pure-local (`cache`), per-request
  PDP (`remote-pdp`), or hybrid (cache decides, PDP audits a sample).
  Latency vs freshness vs auditability is a configuration knob, not a
  rewrite.
- **Closed-loop evidence** — every drift event POSTs to A²D, so the
  approver who signed the descriptor sees the runtime regression next
  to their approval — no Slack thread required.
- **Optional enforcement** — `enforce` mode strips the drifted tool
  from the response *before* the agent reads it. `warn` and `observe`
  are first-class for staged rollouts.

### Who needs this

- Platform teams who let product teams ship MCP servers fast but
  cannot let undocumented descriptor changes reach prod agents.
- Compliance owners who need an audit trail tying *every* runtime
  descriptor back to a named approver and date.
- Anyone running an LLM agent on top of an MCP server they don't
  fully control.

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

### Real-world scenario

A bank ships an "Account Lookup" MCP server. The approved
`lookup_account` tool returns the account holder's name and account
type — that's what compliance signed off on, and the description in
A²D is explicit: *"Returns only non-sensitive account metadata."*

Six weeks later, a well-meaning engineer adds the IBAN to the response
schema "to support a new feature." The MCP server is redeployed. No
ticket, no approval, no PR review by the security team — but the
agent driving customer chat now happily includes IBANs in its
responses, and the chat transcript is logged to a third-party
analytics tool.

With this policy attached:

1. Next `tools/list` to the deployed MCP server is hashed and
   compared against the A²D-approved pin.
2. The `outputSchema` hash diverges — `descriptor_drift` event fires
   with `field: output_schema_changed`.
3. In `enforce` mode, the runtime `lookup_account` is stripped from
   the response before the agent sees it. Customer chat falls back to
   the safe "I don't have access to that information" path instead of
   leaking the IBAN.
4. The drift event lands in A²D next to the original approver's name,
   approval timestamp, and a diff of the schema. Compliance has the
   evidence trail without anyone having to reproduce the bug.

The agent is unchanged. The MCP server is unchanged. The gateway is
the only enforcement point — and the only thing that knew the
descriptor wasn't supposed to grow an `iban` field.

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
