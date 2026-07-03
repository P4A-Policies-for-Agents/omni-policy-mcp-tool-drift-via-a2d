# Demo — MCP Tool Drift Detection (via A²D)

A step-by-step walkthrough of the policy's two paths — **happy** (spec
matches runtime → tools pass through) and **failure** (spec mismatch or
drift → tools stripped in `enforce` mode). The only knob that flips
between them is whether the policy's `a2d.assetId` matches the asset the
gateway is actually proxying.

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
