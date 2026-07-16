# MCP Tool Drift Detection (via A²D)

Detects MCP **tool drift** — a runtime `tools/list` response that
diverges from the spec approved in A²D — and (optionally) strips the
drifted tools from the response before they reach the LLM client. A²D
owns the canonical spec; the gateway is the enforcement point; evidence
flows back to A²D so drift lands next to the approving identity.

> [!IMPORTANT]
> **⚠️ MANDATORY on a managed Omni Gateway — A²D is on a `Host`-routed edge (Vercel).**
>
> A²D ships on `https://www.a2d-ai.com` (hosted on **Vercel**). Vercel — like every multi-tenant edge platform: **Railway, Render, Heroku, Cloudflare Pages/Workers, Fly.io, Netlify** — routes strictly by the HTTP `Host` header / TLS SNI. On a **managed** Omni Gateway (e.g. Anypoint CloudHub 2.0), policy-originated (WASM) outbound calls have their egress `Host` rewritten to an internal Envoy cluster name, so the edge can't find the app and returns `404 DEPLOYMENT_NOT_FOUND` (or `404`/`502`).
>
> **You MUST route the A²D callout through a same-gateway loopback "pin":**
> 1. Set **`a2d.baseUrl`** = `http://127.0.0.1:8081` (the gateway's own internal listener).
> 2. Set **`a2d.pinPathPrefix`** = `/a2d-pin`.
> 3. Create a plain passthrough route (no policy) on the **same** gateway at `/a2d-pin`, upstream = `https://www.a2d-ai.com`, with **`auto_host_rewrite`** so the correct `Host` is restored on egress.
>
> Without the pin the policy **cannot reach A²D** on a managed gateway. Full recipe: [`docs/managed-omni-gateway-setup.md`](docs/managed-omni-gateway-setup.md). The same pin applies verbatim if you self-host A²D (or its mock) on any of the edge platforms listed above.
>
> **Self-managed / connected Flex Gateway** (reaches `www.a2d-ai.com` directly and honors route `auto_host_rewrite`): leave **`a2d.pinPathPrefix`** empty for a direct call.

## What it catches

- **Descriptor drift** — a pinned tool whose runtime descriptor no
  longer hashes to the A²D-approved spec (`description`, `inputSchema`,
  `outputSchema`, or `annotations` changed).
- **Unpinned tools** — present at runtime, absent from the A²D spec.
- **Removed tools** — pinned tools missing at runtime (informational).

## Decision sources

The defining axis of this policy is **where the decision comes from**:

- **`cache`** — decide locally from a refreshed spec cache. Lowest
  latency; staleness bounded by `refreshIntervalSec`.
- **`remote-pdp`** — additionally call A²D's PDP
  (`/api/platform/{assetId}/mcp/validate`) for a central verdict.
- **`hybrid`** — decide locally for latency; audit a deterministically
  sampled fraction of requests against the PDP.

## Decision modes

`enforce` strips drifted tools from the response. `warn` passes through
with drift evidence. `observe` emits evidence only.

## Configuration

| Path | Type | Default | Description |
|---|---|---|---|
| `a2d.baseUrl` | string (`format: service`) | `http://127.0.0.1:8081` | A²D platform base URL. Optional. Loopback default for managed gateways; set `https://www.a2d-ai.com` on self-managed gateways. |
| `a2d.assetId` | string | required | A²D MCP asset id. |
| `a2d.apiKeySecretRef` | string | required | Flex secrets entry with the per-instance A²D policy-scoped API key (`Authorization: Bearer <key>`). |
| `a2d.pinPathPrefix` | string | `""` | Loopback mode (managed gateways) — see below. |
| `a2d.refreshIntervalSec` | int 30–86400 | 300 | Cache-mode spec refresh cadence. |
| `a2d.pdpTimeoutMs` | int 25–5000 | 250 | Per-request PDP timeout. |
| `decision.source` | enum | `cache` | `cache` / `remote-pdp` / `hybrid`. |
| `decision.hybridSampleRate` | float 0–1 | `0.1` | Hybrid PDP audit rate. |
| `enforce.exactMatch` | bool | `true` | Strict hash equality. |
| `enforce.allowAddedTools` | bool | `false` | Allow tools absent from the spec. |
| `enforce.allowRemovedTools` | bool | `true` | Allow pinned tools removed at runtime. |
| `evidence.reportToA2d` | bool | `true` | POST evidence to A²D. |
| `evidence.logLocally` | bool | `true` | Emit JSON log lines. |
| `mode` | enum | `enforce` | `enforce` / `observe` / `warn`. |
| `failOpen.onSpecUnavailable` | bool | `false` | Allow traffic when the spec can't load. |
| `failOpen.onPdpUnavailable` | bool | `true` | Fall back to cache when the PDP is down. |

Required at attach time: `a2d.assetId`, `a2d.apiKeySecretRef`.

## Deploying on a managed Omni Gateway (CloudHub 2.0)

Managed Flex/Omni gateways rewrite the outbound `Host` on
policy-originated calls to an internal cluster name. A²D is hosted on
Vercel, which routes strictly by `Host`, so a direct spec fetch returns
`404 DEPLOYMENT_NOT_FOUND`. This cannot be fixed from policy code.

The supported workaround is a **shared pin-fetch loopback route**: a
plain HTTP passthrough API on the same gateway that points at
`https://www.a2d-ai.com` and carries no policy. The policy calls it
through the gateway's internal listener; the route's host-rewrite
restores the correct A²D `Host`.

1. Create the loopback route (`/a2d-pin/` → `https://www.a2d-ai.com`),
   with the proxy scheme HTTP on port `8081` (TLS terminates at the LB).
2. Configure the policy for loopback mode:

```json
{
  "a2d": {
    "baseUrl": "http://127.0.0.1:8081",
    "assetId": "<a2d-asset-id>",
    "apiKeySecretRef": "<secret-ref>",
    "refreshIntervalSec": 300,
    "pinPathPrefix": "/a2d-pin"
  },
  "decision": { "source": "cache" },
  "mode": "enforce",
  "failOpen": { "onSpecUnavailable": true }
}
```

On self-managed (connected) gateways whose pod can reach A²D directly,
leave `pinPathPrefix` empty and set `baseUrl: https://www.a2d-ai.com`.

Full step-by-step CLI instructions, verification commands, and the
managed-gateway build/deploy gotchas are in the policy repository at
`docs/managed-omni-gateway-setup.md`.

## Verifying enforcement

With a spec that does not match the runtime tools (mismatched asset or a
drifted descriptor), a `tools/list` in `enforce` mode returns an empty
tool set:

```
data: {"jsonrpc":"2.0","id":1,"result":{"tools":[]}}
```

When the runtime descriptors match the spec, the tools pass through
unchanged. `observe` mode never strips and emits evidence only.
