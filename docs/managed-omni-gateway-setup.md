# Deploying on a managed Omni / Flex Gateway (CloudHub 2.0)

This guide covers deploying the **MCP Tool Drift Detection (via A²D)**
policy on a **managed** Anypoint Flex / Omni Gateway (CloudHub 2.0), and
the one non-obvious piece of infrastructure it needs there: an A²D
**pin-fetch loopback route**.

This applies to **any** `Host`-routed multi-tenant edge platform — Vercel, Railway, Render, Heroku, Cloudflare Pages/Workers, Fly.io, Netlify — not only Vercel.

If you are running a self-managed (connected) Flex Gateway where the pod
can reach A²D directly with the correct `Host`, you do **not** need the
loopback — set `a2d.baseUrl` to `https://www.a2d-ai.com`, leave
`a2d.pinPathPrefix` empty, and skip to [Configuration](#configuration).

---

## Why a loopback is required on a managed gateway

The policy fetches its spec (and, in `remote-pdp` / `hybrid` mode, calls
the PDP) over HTTP from `{baseUrl}/api/platform/{assetId}/mcp/{spec,validate,evidence}`.
On a **managed** Omni gateway, the runtime rewrites the outbound `Host` /
`:authority` of any policy-originated (WASM) call to an internal Envoy
cluster identifier.

A²D is hosted on Vercel, which routes **strictly by `Host`**. When the
gateway sends a cluster-name `Host` instead of `www.a2d-ai.com`, Vercel
cannot map it to a project and returns:

```
HTTP 404 {"error":{"code":"404","message":"The deployment could not be found on Vercel."}}
```

This is **not fixable from policy/WASM code**. It was verified that:

- `baseUrl: https://www.a2d-ai.com` (and the bare apex) → 404.
- Re-dispatching through the API's *own* upstream cluster → still 404.
- Disabling the runtime's host rewrite globally
  (`FLEX_REWRITE_HOST_HEADER=false`) fixes WASM egress but **breaks the
  main proxy path**, so the two needs are mutually exclusive globally.

### The fix: call the gateway's own listener instead of A²D

Instead of calling A²D directly, the policy calls a **plain passthrough
route on the same gateway** whose upstream is `www.a2d-ai.com`. That
route is normal proxied traffic, so its `auto_host_rewrite` sets the
correct `Host` for A²D. The policy reaches that route via the gateway's
**internal listener** (`http://127.0.0.1:8081`), which bypasses the
CloudHub load balancer that enforces `Host` at the public edge.

```
          policy (WASM)                      loopback route              A²D
  ┌─────────────────────────┐   http://127.0.0.1:8081/a2d-pin/...   ┌──────────────┐
  │ baseUrl=127.0.0.1:8081   │ ───────────────────────────────────▶ │ Envoy route  │
  │ pinPathPrefix=/a2d-pin   │   Host is mangled here, but Envoy      │ /a2d-pin/ →  │
  └─────────────────────────┘   routes by PATH, not Host             │ auto_host_   │
                                                                      │ rewrite      │
                                                                      └──────┬───────┘
                                                          Host: www.a2d-ai.com│
                                                                             ▼
                                                                 https://www.a2d-ai.com
                                                                 /api/platform/{id}/mcp/spec
```

Why the mangled `Host` stops mattering: a bogus `Host` on the public
edge returns a bare `404 NOT FOUND` **without** the
`server: Anypoint Flex Gateway` header — proving the `Host` check is at
the **CloudHub load balancer**, in front of Envoy. The internal
`127.0.0.1:8081` hop bypasses that LB and hits Envoy directly, where
routing is path-based, so the loopback route's `/a2d-pin/` base path is
matched regardless of the mangled `Host`.

The same loopback route is shared by every A²D policy on the gateway —
it is not specific to this policy.

---

## Step 1 — Create the loopback passthrough route

The loopback is an ordinary HTTP proxy API on the **same** managed
gateway, pointing at `https://www.a2d-ai.com`, carrying **no policy**.

MCP-type Exchange assets reject `--type http`, so first publish a
throwaway `http-api` asset to back it:

```bash
anypoint-cli-v4 exchange asset upload a2d-pin-loopback-api/1.0.0 \
  --name a2d-pin-loopback-api --type http-api \
  --properties '{"apiVersion":"v1"}'
```

Create the instance. On managed CloudHub 2.0 the **proxy** scheme must
be HTTP on port `8081` (TLS terminates at the LB) even though the public
endpoint is HTTPS:

```bash
GW_HOST="agent-network-ingress-gw-<suffix>.<region>.cloudhub.io"
anypoint-cli-v4 api-mgr api manage a2d-pin-loopback-api 1.0.0 \
  --environment "Sandbox" \
  --isFlex -p \
  --type http \
  --uri "https://www.a2d-ai.com" \
  --scheme http --port 8081 \
  --path "/a2d-pin/" \
  --endpointUri "https://$GW_HOST/a2d-pin/" \
  --apiInstanceLabel "a2d-pin-loopback" \
  --deploymentType hybrid
```

Deploy it to the managed gateway target:

```bash
anypoint-cli-v4 api-mgr api deploy <new-instance-id> \
  --environment "Sandbox" \
  --target <gateway-target-id> \
  --gatewayVersion "1.13.2"
```

**Verify** the route reaches A²D (Flex strips the `/a2d-pin/` base path,
so `…/a2d-pin/api/platform/<id>/mcp/spec` proxies to
`www.a2d-ai.com/api/platform/<id>/mcp/spec`):

```bash
curl -s -o /dev/null -w "%{http_code}\n" \
  "https://$GW_HOST/a2d-pin/api/platform/<a2d-asset-id>/mcp/spec" \
  -H "Authorization: Bearer <a2d-key>"
# expect: 200
```

---

## Step 2 — Configure the policy for loopback mode

Set `a2d.baseUrl` to the internal listener and `a2d.pinPathPrefix` to the
loopback route's base path. When `pinPathPrefix` is non-empty the policy
runs in **loopback mode**: it dispatches straight to `baseUrl` (skipping
upstream-cluster discovery) and prefixes every A²D request path.

```json
{
  "a2d": {
    "baseUrl": "http://127.0.0.1:8081",
    "assetId": "<a2d-asset-id>",
    "apiKeySecretRef": "<a2d-policy-scoped-key-or-secret-ref>",
    "refreshIntervalSec": 300,
    "pinPathPrefix": "/a2d-pin"
  },
  "decision": { "source": "cache" },
  "mode": "enforce",
  "failOpen": { "onSpecUnavailable": true }
}
```

Apply it (or update in place with `policy edit`, see below):

```bash
anypoint-cli-v4 api-mgr policy apply <mcp-api-instance-id> \
  omni-policy-mcp-tool-drift-via-a-2-d \
  --environment "Sandbox" \
  --groupId <org-id> \
  --policyVersion <version> \
  --configFile policy-config.json

anypoint-cli-v4 api-mgr api redeploy <mcp-api-instance-id> --environment "Sandbox"
```

---

## Step 3 — Verify the spec loads

After redeploy + ~60 s warmup, a `tools/list` whose runtime descriptors
do **not** match the spec returns an empty tool set in `enforce` mode:

```bash
curl -s -X POST "https://$GW_HOST/<mcp-basepath>/" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","method":"tools/list","id":1}'
# -> data: {"jsonrpc":"2.0","id":1,"result":{"tools":[]}}
```

If tools pass through unchanged in `enforce` mode, the spec did **not**
load and the policy failed open (`failOpen.onSpecUnavailable=true`).
Check Runtime Manager logs for `mcp-drift-a2d: spec fetch failed` and for
the `mcp-drift-a2d: spec loaded (first_load=true …)` line that confirms
the first successful fetch.

---

## Configuration

All standard fields are documented in the [README](../README.md). The
loopback-specific field:

| Path | Type | Default | Description |
|---|---|---|---|
| `a2d.pinPathPrefix` | string | `""` | When set (e.g. `/a2d-pin`), enables loopback mode: dispatch A²D calls to `baseUrl` verbatim (no cluster discovery) with this prefix prepended to every request path. Leave empty for a direct A²D call on gateways that don't mangle the egress `Host`. |

---

## Operational notes (managed gateway)

### Updating config without the flaky `policy remove`

On this managed target, `policy remove` intermittently returns
`There was an error while talking to Configuration Manager`, which then
blocks re-apply with `A Resource Level Policy is already applied`. Update
the config in place instead:

```bash
anypoint-cli-v4 api-mgr policy edit <mcp-api-instance-id> <policy-id> \
  --environment "Sandbox" --configFile policy-config.json
anypoint-cli-v4 api-mgr api redeploy <mcp-api-instance-id> --environment "Sandbox"
```

### The `CARGO_TARGET_DIR` build trap (read before publishing)

If a "new" policy version behaves exactly like the old one no matter how
many times you redeploy or restart the gateway, you are almost certainly
publishing a **stale wasm**. Some sandboxes override `CARGO_TARGET_DIR`,
so `cargo build` writes the fresh wasm to a cache dir while `make
publish` uploads the old binary still sitting in `./target`.

Always build + publish with a consistent target dir, and grep the binary
for a marker only the new code has **before** publishing:

```bash
env -u CARGO_TARGET_DIR make publish
LC_ALL=C grep -a -c "pinPathPrefix" \
  target/wasm32-wasip1/release/omni_policy_mcp_tool_drift_via_a2d.wasm   # 0 == stale
echo "$CARGO_TARGET_DIR"   # must be empty, or ./target
stat -f '%Sm' target/wasm32-wasip1/release/*.wasm   # mtime must be "now"
```

See [`DEPLOYMENT-NOTES.md`](../DEPLOYMENT-NOTES.md) for other
managed-gateway gotchas (trailing-slash routing, the MCP `routing[]`
shape, invisible gateway-runtime registration).

---

## How the happy path and failure path are simulated

The policy's verdict is a comparison: **does each runtime tool
descriptor match the A²D spec?** So a demo just needs to control whether
the proxied MCP server's `tools/list` matches the pinned asset.

### Failure path — spec/runtime mismatch (the enforcement demo)

Point the policy's `a2d.assetId` at a **different** A²D asset than the
one the gateway proxies. Every runtime tool is then **absent from the
spec** → classified `unpinned_tool` → stripped in `enforce` mode:

```
tools/list -> {"result":{"tools":[]}}
```

The same effect occurs for a matching asset whose runtime descriptor has
**drifted** from its approved snapshot (the real-world poisoning case: an
attacker mutates a description after approval → `descriptor_drift` →
stripped).

### Happy path — spec matches runtime

Point `a2d.assetId` at the asset the gateway actually serves so the
runtime descriptors match the spec; the tools pass through untouched:

```
tools/list -> {"result":{"tools":[{"name":"fetch_weather"...},...]}}
```

`observe` mode is also a valid "does not disrupt traffic" demonstration —
it never strips, regardless of drift, and emits evidence only.

### A real caveat on this A²D deployment

A²D exposes two endpoints per asset that are serialized **differently**:

- `/api/platform/{id}/mcp/spec` — the approved **spec** snapshot. Its
  `inputSchema` has keys `type, required, properties`.
- `/api/platform/{id}/mcp` — the live MCP server `tools/list`. Its
  `inputSchema` is enriched with `additionalProperties` and `$schema`.

Because the canonical hash covers `inputSchema`, the *same* asset can
still register as `descriptor_drift` (the schema envelopes differ), so a
naive same-asset "happy path" strips every tool in `enforce` mode. For a
byte-clean happy path, use an MCP mock whose live `tools/list` matches
its approved spec byte-for-byte (after canonical key-sorting), or A²D
endpoints that emit identical schema serialization for the asset.
