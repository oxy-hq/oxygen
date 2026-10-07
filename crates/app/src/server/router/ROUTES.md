# Router module guide

Reference for the HTTP surface in [`crate::server::router`]. Everything below is
mounted under `/api` by [`crate::cli::commands::serve`].

## Module layout

| Module | Contents |
|---|---|
| [`mod.rs`](./mod.rs) | `AppState`, `WorkspaceExtractor`, shared `build_cors_layer`, router tests |
| [`entry.rs`](./entry.rs) | `api_router` / `internal_api_router` — assembles the full router, applies CORS, timeout, Sentry |
| [`public.rs`](./public.rs) | Routes with no *user session* gate (health, auth handshake, current user, and webhook receivers — Slack, Toast, and `POST /webhooks/apps/{org}/{app}/{fn}`, which authenticates the sender by HMAC instead) |
| [`global.rs`](./global.rs) | Cloud-only flat routes (logout, org CRUD, per-user GitHub) |
| [`workspace.rs`](./workspace.rs) | The `/{workspace_id}/…` tree and every per-resource sub-builder |
| [`secrets.rs`](./secrets.rs) | Secret CRUD + the admin-only gating middleware |
| [`protected.rs`](./protected.rs) | Cloud/local composition: which route sets are mounted and which middleware wraps them |
| [`openapi.rs`](./openapi.rs) | Curated `utoipa` router used by Swagger UI |

## Middleware stacks

Both modes share the same outer wrapping from `entry.rs`:

```
CORS → global 60s timeout → Sentry → <router>
```

The protected-route inner stack differs by mode:

- **Cloud** (`apply_middleware`): `auth_middleware(AuthState::built_in)` → `timeout_middleware`
  then `workspace_middleware` on every `/{workspace_id}/…` request.
- **Local** (`apply_local_middleware`): `auth_middleware(AuthState::guest_only)` → `timeout_middleware`
  then `local_context_middleware` on every `/{workspace_id}/…` request.

`build_global_routes` (org routes) is **not** mounted in local mode.

Inside the cloud auth layer, `token_grant_scope_middleware` answers 404 to a grant-bound API
token (`all_access = false`) on every flat route that cannot honour its grants. The honoured
set is the `treatment` match in `api/middlewares/token_grant_scope.rs`; a new flat route is
refused until it is listed there. An all-access token that an org has blocked keeps the flat
routes and loses that org's data only: `WHEN_BLOCKED` in the same file says, route by route,
how each membership-keyed handler leaves the blocked org out, and a route missing from it is
refused to such a token. Sessions, legacy keys and all-access tokens no org has blocked are
untouched.

The `/orgs/{org_id}/github/*` and `/user/github/*` subtrees below are no longer
defined here: they live in the sibling `oxy-api-github` crate and are injected by
the composition root (`oxy-server`) through `api_router`'s `SurfaceSeams::api` seam,
merged into the protected tree before `apply_middleware` (cloud mode only). They
re-apply their own `org_middleware` + `subscription_guard`; oxy-app no longer
depends on them.

## Route tree

The tree below is a hand-written orientation map — read it to learn the *shape*
of the surface and where a new route belongs. It is **not** the authoritative
list: for that, run `oxyc routes` (or `oxyc routes --json`), which
prints every endpoint the binary mounts from a catalog
`crates/route-catalog/build_route_catalog.rs` extracts from these very files at
build time. `server::route_catalog` holds the types and search; the completeness
tests live beside the table in `oxy-route-catalog`.

Legend: `🌐` public · `☁️` cloud only · `🏢` cloud + local (per-workspace)

### 🌐 Public (always mounted)

```
GET    /health  /ready  /live  /version
GET    /auth/config
POST   /auth/google  /auth/github  /auth/okta
POST   /auth/magic-link/request  /auth/magic-link/verify
POST   /auth/cli/exchange        (`oxyc login`: redeem the one-time code for a token)
POST   /auth/browser-ticket/redeem (`oxyc login-link`: a browser redeems a token's one-time ticket for a session that acts as that token)
POST   /auth/oidc/exchange       (trusted access: a GitHub Actions OIDC token for a 15-minute `oxy_ci_` token; rate-limited per client)
POST   /auth/tokens/revoke-leaked (leak response: revoke reported new-format tokens, never a legacy key; rate-limited per client)
GET    /user
GET|POST /auth/dev-login          (404s unless OXY_DEV_LOGIN_EMAILS is set)
```

### ☁️ Global — cloud only

```
GET    /logout
GET    /orgs                                  (no POST: staff/partners create orgs)
POST   /invitations/{token}/accept

/orgs/{org_id}/                              (org_middleware)
├── GET / · PATCH / · DELETE /
├── GET /members
├── PATCH  /members/{user_id}
├── DELETE /members/{user_id}
├── GET /invitations · POST /invitations
├── DELETE /invitations/{invitation_id}
├── POST /onboarding/demo · /onboarding/new · /onboarding/github
├── GET /workspaces
├── DELETE /workspaces/{id}
├── PATCH  /workspaces/{id}/rename
├── GET|PUT /token-policy                    (org admin; PUT session-only — API-tokens Phase 5)
└── /github/
    ├── GET /repositories · /branches · /namespaces
    ├── POST /namespaces/pat · /namespaces/installation
    └── DELETE /namespaces/{id}

/user/github/
├── GET  /account · DELETE /account
├── GET  /account/oauth-url
├── GET  /installations · /installations/new-url
└── POST /callback

/user/tokens/                                 (session only — a token cannot manage tokens)
├── GET  /   · POST /                         (POST with `kind: "sandbox_agent"` mints an `oxy_sbx_` token: staff only, per app)
├── GET  /{id} · PATCH /{id} · DELETE /{id}
├── POST /{id}/extend · /{id}/regenerate      (PATCH, extend and regenerate answer 409 for a sandbox agent token)
└── GET  /{id}/activity
GET    /user/token-options                    (session only)
GET|DELETE /auth/token                        (the calling token, about itself)
POST   /auth/cli/authorize                    (session only — `oxyc login`: a one-time code for the CLI's challenge; with `mint`, a code for a sandbox agent token)
POST   /auth/browser-ticket                   (a personal token only — `oxyc login-link`: a one-time ticket that signs a browser in as the calling token)

/admin/sandbox-agent-tokens                   (`operate_platform`; rows narrowed to the grant's orgs)
├── GET  /
└── POST /{id}/revoke
```

### 🏢 Workspace — `/{workspace_id}/…`

Mounted in both modes. Cloud uses the real workspace UUID; local always uses
`LOCAL_WORKSPACE_ID` (nil UUID).

```
/{workspace_id}/
├── Git / workspace ops
│   ├── GET    /details · /status · /revision-info
│   ├── GET    /branches
│   ├── DELETE /branches/{branch_name}
│   ├── POST   /switch-branch · /pull-changes · /push-changes · /force-push
│   ├── POST   /abort-rebase · /continue-rebase
│   ├── POST   /resolve-conflict-file · /unresolve-conflict-file · /resolve-conflict-with-content
│   ├── GET    /recent-commits
│   └── POST   /reset-to-commit
│
├── /workflows/          list, get, run, run-sync, logs, runs CRUD, bulk-delete
├── /automations/save
├── /threads/            list, create, delete-all, bulk-delete, get, delete,
│                        task, agentic, workflow, workflow-sync, messages, agent, stop
├── /agents/             list, get, ask, ask-sync, run-test
├── /api-keys/           list, create, get, delete, extend, activity
├── /api-tokens          the tokens that can reach this workspace (read-only, admin)
├── /files/              tree, diff-summary, get, from-git, revert, save,
│                        delete(-file|-folder), rename-(file|folder), new-(file|folder)
├── /databases/          list, create, test-connection, sync, build, clean
├── /repositories/       list, add, remove, branch ops, diff, commit, files, github
├── /integrations/       looker: list, query, query/sql
├── /secrets/            list, create, bulk, env, get, update, delete, reveal  (admin-gated)
├── /tests/              test files, project-runs, runs + human-verdicts
├── /apps/               list, get, run, result, displays, charts, file, source, save-from-run
├── /traces/             traces_routes()
├── /metrics/            metrics_routes()
├── /execution-analytics/
├── /analytics/          agentic_router() (chart / app-builder pipeline)
│
├── /members · /members/{user_id}         (put / delete role overrides)
├── /artifacts/{id}
├── /charts/{file_path}
├── /exported-charts/{file_name}
├── /logs
├── /events · /events/lookup · /events/sync
├── /blocks
├── /runs/{source_id}/{run_index}          (cancel)
├── /builder-availability · /onboarding-readiness
├── /sql/{pathb64} · /sql/query
├── /semantic · /semantic/compile · /semantic/topic/{file_path_b64} · /semantic/view/{file_path_b64}   (compile + execute go through airlayer + agentic-connector)
└── /results/files/{file_id}               (get, delete)
```

## Where to add a new route

1. **Per-workspace resource** → add a builder in `workspace.rs` and nest it in
   `build_workspace_routes`. It will automatically be available in both cloud
   and local modes.
2. **Org-level / cloud-only** → add it in `global.rs`. It will not be mounted
   in local mode.
3. **No auth required** → add it in `public.rs`. It is mounted in both modes.
4. **Admin-only** → if it operates on secrets, add it in `secrets.rs`; otherwise
   add your own middleware alongside an existing route group.
