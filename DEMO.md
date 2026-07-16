# Demo — MCP Tool Drift Detection (via A²D)

A step-by-step walkthrough of the policy's two paths — **happy** (spec
matches runtime → tools pass through) and **failure** (spec mismatch or
drift → tools stripped in `enforce` mode). The only knob that flips
between them is whether the policy's `a2d.assetId` matches the asset the
gateway is actually proxying.

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

> All curls use a **trailing slash** on the MCP base path — the Flex
> Gateway routes the proxy path as an exact prefix including the slash.
> See `DEPLOYMENT-NOTES.md`.

---

## 0. Prerequisites

- The policy is published and attached to an MCP-type API instance on a
  managed Omni gateway.
- The shared `/a2d-pin` loopback route exists (see
  [`docs/managed-omni-gateway-setup.md`](docs/managed-omni-gateway-setup.md)).
- `a2d.apiKeySecretRef` resolves to a real A²D **policy-scoped** API key
  (a placeholder still lets the proxy come up, but the spec never loads
  and the policy fails open).

Environment for the snippets below:

```bash
GW_HOST="agent-network-ingress-gw-<suffix>.<region>.cloudhub.io"
MCP_BASE="/mcp-drift-via-a2d-demo"     # your MCP instance's base path
A2D_KEY="<a2d-policy-scoped-key>"
```

---

## 1. Base config (loopback mode, enforce)

`policy-config.json` in the repo root is a ready-to-apply loopback
config. The two demo-relevant fields are `a2d.assetId` and `mode`:

```json
{
  "a2d": {
    "baseUrl": "http://127.0.0.1:8081",
    "assetId": "<ASSET_ID>",
    "apiKeySecretRef": "<secret-ref>",
    "refreshIntervalSec": 300,
    "pinPathPrefix": "/a2d-pin"
  },
  "decision": { "source": "cache" },
  "mode": "enforce",
  "failOpen": { "onSpecUnavailable": false }
}
```

Apply + redeploy after any config change:

```bash
anypoint-cli-v4 api-mgr policy edit <mcp-api-instance-id> <policy-id> \
  --environment "Sandbox" --configFile policy-config.json
anypoint-cli-v4 api-mgr api redeploy <mcp-api-instance-id> --environment "Sandbox"
# wait ~30–60s for warmup
```

---

## 2. Happy path — spec matches runtime

Set `a2d.assetId` to **the same** asset the gateway proxies, so the
runtime descriptors match the approved spec.

```bash
curl -sS -X POST "https://$GW_HOST$MCP_BASE/" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","method":"tools/list","id":1}'
```

Expected — every tool passes through unchanged:

```
data: {"jsonrpc":"2.0","id":1,"result":{"tools":[
  {"name":"fetch_weather","description":"...","inputSchema":{...}},
  {"name":"assist_user","description":"...","inputSchema":{...}}
]}}
```

No `descriptor_drift` / `unpinned_tool` evidence fires; the response is
byte-identical to the upstream MCP server's.

---

## 3. Failure path A — mismatched asset (unpinned tools)

Point `a2d.assetId` at a **different** A²D asset than the one the gateway
proxies (e.g. proxy serves a weather/assist server while the spec is
fetched for an *accounts* asset). Every runtime tool is now absent from
the spec → `unpinned_tool` → stripped in `enforce` mode.

```bash
curl -sS -X POST "https://$GW_HOST$MCP_BASE/" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","method":"tools/list","id":1}'
```

Expected — empty tool set:

```
data: {"jsonrpc":"2.0","id":1,"result":{"tools":[]}}
```

Runtime Manager logs show one debounced evidence line per tool:

```
mcp-drift-a2d-evt {"class":"unpinned_tool","decision":"stripped","source":"cache",...}
```

---

## 4. Failure path B — descriptor drift (the real attack)

Keep `a2d.assetId` matching the proxied asset, but mutate one tool's
description in the A²D asset (or upstream MCP server) so the runtime
descriptor no longer hashes to the approved spec. That tool alone is
stripped; untouched tools pass through.

```
data: {"jsonrpc":"2.0","id":1,"result":{"tools":[
  {"name":"assist_user",...}          // clean tool kept
]}}                                    // drifted fetch_weather stripped
```

Evidence:

```
mcp-drift-a2d-evt {"class":"descriptor_drift","severity":"critical","decision":"stripped",...}
```

This is the canonical **failure** path: a tool whose descriptor drifted
from the approved spec is stripped in `enforce` with a
`descriptor_drift` evidence event. Under `decision.source=remote-pdp`
the same drift is blocked **per request** by the live PDP verdict (no
dependence on cache freshness) — see [§6](#6-decision-source-variants).
The **happy** path in [§2](#2-happy-path--spec-matches-runtime) is the
mirror image: a `tools/list` that matches the approved spec passes
through untouched with no evidence.

---

## 5. Non-disruptive verification — observe mode

To prove the detection without changing traffic, set `mode: "observe"`
and re-run the failure path. Tools are **not** stripped, but evidence is
still emitted — ideal for staged rollout before flipping to `enforce`.

```bash
# mode=observe in policy-config.json, then re-apply + redeploy
# tools/list returns all tools; logs still show the drift evidence line.
```

---

## 6. Decision-source variants

- `decision.source: cache` (default) — enforcement is driven entirely by
  the locally cached A²D spec. Lowest latency.
- `decision.source: remote-pdp` / `hybrid` — enforcement still uses the
  local spec diff, and the policy **additionally** calls the A²D PDP
  (`/mcp/validate`) best-effort for a central verdict. In `hybrid` mode
  the PDP audit fires only for the deterministically sampled fraction
  (`hybridSampleRate`). Look for:

```
mcp-drift-a2d: pdp verdict kept=<n> blocked=<n> ver=<asset-version>
```

---

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| Tools pass through in `enforce` even with a mismatched asset | Spec never loaded → policy failed open. Check for `mcp-drift-a2d: spec loaded` vs `spec fetch failed` in logs; verify the `/a2d-pin` loopback route returns 200. |
| `404` on the demo curl | Missing trailing slash, or MCP `routing[]` misconfigured — see `DEPLOYMENT-NOTES.md`. Not caused by a placeholder secret. |
| "New" version behaves like the old one | Stale wasm from the `CARGO_TARGET_DIR` trap — grep the binary for `pinPathPrefix` before publishing (see the managed-gateway doc). |
