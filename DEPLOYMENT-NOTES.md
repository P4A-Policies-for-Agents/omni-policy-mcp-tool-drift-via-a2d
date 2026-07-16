# Deployment notes — Anypoint Omni Gateway

Gotchas that took an entire debugging session to find while bringing
this policy up on `agent-network-ingress-gw` (org
`82a0453b-22e6-430d-bbf4-35b989d043dc`, env `Sandbox`). Read before
deploying or recreating instances.

---

> **⚠️ MANDATORY on a managed Omni Gateway — READ FIRST (`Host`-routed edge / pin).**
>
> A²D ships on `https://www.a2d-ai.com` (hosted on **Vercel**). Every multi-tenant edge
> PaaS — **Vercel, Railway, Render, Heroku, Cloudflare Pages/Workers, Fly.io, Netlify** —
> routes strictly by the HTTP `Host` header / TLS SNI. On a **managed** Omni Gateway
> (Anypoint CloudHub 2.0), policy-originated (WASM) outbound calls have their egress
> `Host` rewritten to an internal Envoy cluster name, so the edge returns
> `404 DEPLOYMENT_NOT_FOUND` (or `404`/`502`). This is the *actual* root cause behind the
> "unreachable A²D / closed-network" note below — it is not a firewall.
>
> **You MUST route the A²D callout through a same-gateway loopback "pin":**
> 1. `a2d.baseUrl` = `http://127.0.0.1:8081` (the gateway's own internal listener).
> 2. `a2d.pinPathPrefix` = `/a2d-pin`.
> 3. Add a plain passthrough route (no policy) on the **same** gateway at `/a2d-pin`,
>    upstream `https://www.a2d-ai.com`, with **`auto_host_rewrite`** so the correct
>    `Host` is restored on egress.
>
> Full walkthrough: [`docs/managed-omni-gateway-setup.md`](docs/managed-omni-gateway-setup.md)
> and the callout at the top of [`README.md`](README.md). Self-managed / connected Flex
> Gateway that reaches A²D directly: leave `a2d.pinPathPrefix` empty for a direct call.

## Trailing slash is required on the proxy path

The Flex Gateway routes the proxy path **as exact prefix including a
trailing slash**. An instance configured with path `/foo` will only
answer at `/foo/`. Without the trailing slash the gateway returns
`HTTP 404` with `server: Anypoint Flex Gateway` and an empty body.

Always set:

- `endpointUri` → `https://<host>/<basepath>/` (trailing slash)
- `--path` → `/<basepath>/` (trailing slash)
- curl URL in demos → `<host>/<basepath>/` (trailing slash)

## Do NOT set `routing[0].rules.path` for MCP-type instances

The Anypoint UI creates MCP instances with a routing block of the form:

```json
"routing": [
  { "upstreams": [ { "id": "<upstream-uuid>", "weight": 100 } ] }
]
```

There is **no** `label` and **no** `rules.path` field. The basepath
lives in `endpoint.proxyUri` / `--path`, not in `routing[].rules.path`.

If you set `routing[].rules.path` via `api-mgr api edit --routing`, the
Anypoint API record validates but the gateway-runtime application
either fails to register or registers without the route, and every
request returns 404 with no upstream traffic.

**Correct edit command:**

```bash
anypoint-cli-v4 api-mgr api edit <id> \
  -f \
  --path "/<basepath>/" \
  --endpointUri "https://<host>/<basepath>/" \
  --routing '[{"upstreams":[{"id":"<upstream-uuid>","weight":100}]}]'
```

After every edit, run `anypoint-cli-v4 api-mgr api redeploy <id>`.

## Placeholder `apiKeySecretRef` does NOT block the proxy

The policy ships with `apiKeySecretRef: REPLACE_WITH_PLATFORM_API_KEY`.
The proxy still comes up, the policy attaches, and `tools/list` still
flows through — the policy just fails to fetch the spec (no key → no
spec → `SpecUnavailable` evidence fires and `failOpen.onSpecUnavailable`
controls allow/block).

So: **a 404 on the demo is never caused by the placeholder secret.**
Look elsewhere first.

Before the policy actually enforces, swap the placeholder for a real
Anypoint Secret Manager reference. The secret value is an A²D
**policy-scoped** API key for the relevant org / asset — the policy
sends it as `Authorization: Bearer <key>` to reach `/mcp/spec`,
`/mcp/validate`, and `/mcp/evidence` only. Do NOT use a full-scope
A²D user token.

## `remote-pdp` mode needs a reachable A²D validate endpoint

`decision.source: remote-pdp` calls
`{baseUrl}/api/platform/{assetId}/mcp/validate` on every request. If
A²D is unreachable and `failOpen.onPdpUnavailable=true` (default) the
policy falls back to the cached spec; otherwise the response is
blocked. In closed-network deployments where the gateway can't reach
`https://www.a2d-ai.com` directly, either set the fail-open flag or
run the policy in `cache` mode. **On a managed Omni Gateway the usual
cause is the `Host`-rewrite issue (see the pin callout at the top), not a
firewall — fix it with the loopback pin instead of disabling enforcement.**

## `hybrid` mode's sample rate applies per-request, not per-tool

`decision.hybridSampleRate=0.1` means ~10% of requests trigger an async
PDP audit — NOT that 10% of tools get audited. The FNV-1a hash is
seeded by the request correlation id, so a hot tool doesn't
repeatedly tax the PDP.

## `api-mgr api list` may return empty even when instances exist

In Sandbox with API Manager v2, `anypoint-cli-v4 api-mgr api list`
intermittently returns zero rows even when `api describe <id>` succeeds
for every id. Fall back to `describe` per-id; record the ids in this
repo's `Live Demo` table.

## ANSI color codes break JSON parsing

`anypoint-cli-v4 ... -o json` emits ANSI color escapes in some output
paths. Strip with `sed -e 's/\x1b\[[0-9;]*m//g'` before feeding to
`python3 -m json.tool` or `jq`.

## Gateway-runtime registration is invisible from CLI

`anypoint-cli-v4 runtime-mgr application list` does **not** show the
per-API-instance gateway applications. The only authoritative view is
the Anypoint Console:

> Runtime Manager → Omni Gateway → `agent-network-ingress-gw` →
> Applications tab

---

## Order of operations for a clean recreate

1. Create API instance via Anypoint Console UI (API Manager → Add API
   → From scratch → MCP type). The UI sets up `routing` correctly.
2. Set Implementation URI to the upstream MCP server.
3. Set Consumer endpoint with trailing slash.
4. Pick the existing managed gateway target.
5. Apply this policy via `api-mgr policy apply <api-id> <policy-asset-id>
   --policyVersion <version> --configFile policy-config.yaml`.
6. Verify with the curl in `README.md` → "Live Demo" → "Try it".
