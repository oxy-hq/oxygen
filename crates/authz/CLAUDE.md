# `oxy-authz` — the authorization decision layer (`crates/authz`)

**One place states who may do what.** Every authorization decision in Oxy resolves to one
arm of a single `match` in `allows()`. A rule that is not stated there is not a rule.

Authentication (who you are) is `oxy-auth`. This is authorization (what you may do).

- **Why it exists:** authentication was a layer; authorization was a *scatter*. Once the
  extractor handed a handler a `User`, ~170 call sites decided access ad hoc, and the
  copies drifted from their siblings. Writing the model down and differencing it against
  the shipped checks found five real bugs — see the PR body and the design doc.
- **Why there is no policy engine:** this ran on Cedar and the engine was removed. The
  crate header (`src/lib.rs`) records the reasoning; don't relitigate it from scratch.
  Short version: policy-as-data is an explicit non-goal (design §2), and that is the
  requirement that pays for an engine. Adopt one when policy must be authored by someone
  outside this repo — not to compute `contains`.
- **Design:** the model + rejected-engines rationale are captured in this guide and
  `src/lib.rs`; the original unification design doc's history is in git.

## The one boundary that matters: model vs facts

| | Lives in | Says |
| --- | --- | --- |
| **The model** | `oxy-authz` (this crate) | what a principal is *entitled to* |
| **The facts** | `oxy-server-authz` (`loader`) | what is *true of* the principal |

The loader stays in `oxy-server-authz` (`oxy-app`'s `server::authz` until it was extracted)
because it reaches app primitives — org membership, partner
standings, the `app_admins` table. This crate depends on `uuid` + `tracing` and nothing
else, which is what lets the whole model be tested without a database.

**Don't add a DB, HTTP, or `entity` dependency here.** A rule that needs a query is a sign
the fact is missing from `PrincipalFacts`, not that the crate needs a connection.

## Vocabulary

- **`Action`** (63) — the closed vocabulary of things a caller can do. This is what call
  sites name.
- **`Ring`** — the authority level that gates an action. **Private on purpose:** a
  ring is how the model is *stated*, not a menu callers pick from. Public, it would let a
  call site choose its own authority level — the scatter this crate exists to end.
- **`PrincipalFacts`** — the whole input surface. Empty = denied everything (fail closed).
- **`Resource`** — what's being acted on: `org_id`, `kind`, optional `owner`, optional
  acting `partner`.
- **`Cap`** (13) — a capability. **One vocabulary, two tiers:** the first eight are the
  partner ceiling (one-to-one with `PartnerCapability`); `ViewTenants` / `ManagePartners` /
  `OperatePlatform` / `ManagePlatformGrants` / `CheckApps` are platform-only and have no
  partner analogue, deliberately.
- **`Scope`** — `All` or `Orgs(..)`. Where a grant reaches.
- **Principal and token vocabulary** (`src/principal.rs`) — `Identity`, `StoredKind`,
  `AccountStanding`, `AppPublishGrant`, `AppSandboxGrant`: plain data plus pure methods,
  moved from `oxy-auth` so a contract crate can name them from the kernel. Building them
  from rows stays in `oxy-auth`, and so do `CredentialContext` (it holds a `chrono`
  expiry) and `AuthenticatedUser` (it holds an `entity` status).
- **`PlatformStanding`** / **`PlatformRole`** — Oxy-staff standing as `(caps × scope)`, and
  the presets (`GlobalAdmin`, `AppOperator`, `AppChecker`) that name common ones.
  `AppChecker` is `CheckApps` alone: the promote checks' service token. It reads an app's
  **staging** (`Action::AppCheck`, `Ring::StagingCheck`) and its **production**
  (`Action::AppCheckProduction`, `Ring::ProductionCheck`): pages and the app's own data
  reads, never `/fn`, never a write on the app. And it records what the build's checks
  found, through the five runner routes and no other write, a token at an admin ceiling.
  It is not grantable to a person (`PlatformRole::is_grantable`) and not staff at the
  console door (`PrincipalFacts::checks_apps_only`).

Rings, briefly: `Read` · `MemberStrict` · `OrgAdmin` · `OrgAdminStrict` · `OwnerOnly` ·
`OrgAdminOrCreator` · `WorkspaceAdmin` · `WorkspaceAdminStrict` · `WorkspaceEdit` ·
`AppAccess` · `WorkspaceData` · `AppAdmin` · `AppGrant` · `PartnerCap` · `StaffReach` · `StagingCheck` · `ProductionCheck` · `PromoteOverride` · `Factory` · `CompileCheck` · `PlatformAny` ·
`PlatformCap` · `GlobalOwnerOnly`. `StaffReach` is `PlatformCap` on a *tenant* resource: a staff-only
tool used inside one org (non-production app hosts, workspace previews), so the grant's scope is
consulted.

`Factory` (`Action::FactoryUse`) is the one ring where a staff term and tenant terms meet on a
tool: Oxy staff over the org, **or** an owner or admin of the org (a person, at an admin
ceiling; decided 2026-10-27), **or** a real member holding a per-workspace row in
`workspace_factory_grants` (`PrincipalFacts::factory_workspaces`). It reads no workspace role,
so a plain org member is refused without the grant, whatever their workspace role. The routes
it guards, the switch and the operator steps are in `internal-docs/roles-and-authorization.md`
§ The Factory.

`CompileCheck` (`Action::CompileCheck`, decided 2026-11-21) is the customer CI compile gate:
queue a staging compile of a branch and read its status. For a person it is exactly
`FactoryUse` AND `WorkspaceEdit` (what the two routes asked before); the one caller it adds is
the org's service account acting through a **CI run token**, read from the fact
`TokenReach::ci_run`, at any ceiling over the workspace (`viewer` included). It is also the
Factory gate's one exception, on two exact routes. Routes, guard and trust policy:
`internal-docs/roles-and-authorization.md` § The one exception: checking a compile.

`ProductionCheck` (`Action::AppCheckProduction`, decided 2026-11-22) is the checker on a
custom app's **production**: staff holding `CheckApps` over the app's org, and only when
the decision names `EnvFacet::Production`. It is `StagingCheck`'s twin and a ring of its
own for the same reason: the model states where the checker reaches, one environment per
ring, so admitting it to production is an action a call site names, never a facet it
passes. It is asked BESIDE what production already asks, at three doors only
(`oxy_custom_apps::app_check`): the bundle path's resolver (`resolve_page_request`: the
page, its assets, the beacon), the data plane's read gate (`/query`, `/semantic-query`),
and the runner routes of a production attempt. Production's other doors share
`resolve_app_request` and never ask it, so holding it opens no function call, log, health
or debug read. No tenant standing reaches it, and no partner.

`PromoteOverride` (`Action::AppPromoteOverride`, decided 2026-11-22) is who may promote a
custom app's build past its promote checks (`override_checks`, with a reason that is
audited): a **real** owner or admin of the app's org, or Oxy staff reaching in through the
operator override with `ManageApps`, the capability a promote already needs. It is the
officer term of `OrgAdminStrict` beside the staff term of the override rings, which no
other ring states: `OrgAdmin` and `AppGrant` admit the org's admin service account and a
partner, `AppAdmin` an `app_members` admin. So a service account (and a CI run token,
which acts as one), a partner and an app's own admin never hold it, and a staff member who
is a plain member of the org does not either. A legacy publish token carries no narrowing
fact, so the call site (`custom_apps_promote::checks_override`) refuses it by its marker.
Contract: `internal-docs/custom-app-sandboxes.md` section 5.9.

## Staff standing is a grant, not a flag

`is_global_admin: bool` is gone. It was shared by nine tenant rings, so `Ring::OwnerOnly`
— org **deletion** — honoured the same term `Ring::AppAdmin` did, and every app publisher
could delete any tenant. Each ring now names the capability its own authority is about,
via `PrincipalFacts::platform_grants(cap, org)` — the deliberate mirror of
`any_partner_grants`. `is_global_owner` stays a boolean: it is root.

**Capabilities gate verbs; scope filters rows.** `Resource::platform()` has a nil org, so
no platform ring consults scope — a scoped operator passes the console door and the
*handler* narrows what it returns. Getting this backwards yields either a role that 403s
out of its own console or one that lists every tenant. Full guide:
`internal-docs/roles-and-authorization.md`.

**Delegation is the one question that is about relative standing.** `may_delegate` — not
`allows()` — decides who may write a grant row: strictly-lower role, and a scope your own
contains. It is the only place `PlatformRole::rank` is read; comparing ranks anywhere else
reintroduces the "one boolean, nine rings" collapse this model replaced. The capability
(`ManagePlatformGrants`) is the door; this is the fence. Full rationale:
`internal-docs/roles-and-authorization.md`.

The `*Strict` variants reject the global-operator override. That distinction is
load-bearing and has already been got wrong once (billing was modeled `OwnerOnly`; the
real gate is a *real* owner/admin with the override rejected).

## Entry points — pick the right one

| Call | Decision | Use when |
| --- | --- | --- |
| `enforce(label, facts, action, resource, existing_allow)` | `existing_allow && allows(..)` | **The default.** A shipped check exists to difference against. |
| `require(facts, action, resource)` | `allows(..)` | No legacy check exists (a new surface), or its legacy term was retired. |
| `authorize(..)` | `allows(..)`, legacy observed only | **Currently unwired, deliberately.** Drops the fail-safe. |

From the app side, prefer the wrappers in `oxy-server-authz`: `enforce_guard` (in a guard,
memoized facts), `enforce_for` (a call site holding a DB handle + identity),
`partner_allows` (the partner tier).

### `existing_allow` is the oracle, not ceremony

The conjunction is the whole safety property: the model can only ever **subtract** access
the existing check granted, so a mis-modeled ring **cannot open a hole**. The residual
failure is a wrong *deny* — loud (a 403), attributable (a WARN naming the label),
revertible in one line.

Passing a hand-waved `true` silently converts a fail-safe into a bare `allows` **and**
throws away the oracle the differential tests difference against. If there's genuinely no
existing check, use `require` and say so.

## Adding an `Action`

1. Add the variant + a doc comment saying **who** may do it and **why** — including who
   deliberately may *not* (the override, a partner, staff).
2. Add it to `Action::ALL` and give `as_str` a stable id. That id is a **wire contract** —
   it lands in the `authz` tracing output.
3. Map it to a `Ring` in `ring()`. Skipping this fails the build; that exhaustiveness is
   the point of the enum.
4. Add a case to `oxy_server_authz::differential` asserting the ring matches the shipped
   check across every caller shape. Reuse an existing ring rather than inventing one —
   two rings that mean the same thing is how drift restarts.
5. Write its arm in `sandbox_agent::covers`, which will not compile without one. That
   match has no wildcard on purpose, and the arm to write is `false`: a sandbox agent
   token (`oxy_sbx_`) covers four actions, and a new one is not a fifth by default.

## A credential narrows as a fact, never as a ring

(One credential fact widens instead of narrowing, and only inside one ring:
`TokenReach::ci_run`, read by `Ring::CompileCheck` alone, with a sweep proving it moves no
other decision — `compile_check_tests::the_ci_run_fact_moves_no_decision_but_the_compile_check`.)


An API token is the same principal seen through a narrower credential, so it is a fact
on the principal (`PrincipalFacts::token`) that `allows()` reads **first** and can only
subtract with — not an `Action`, and not a `Ring`. The sandbox agent token is the
narrowest: `TokenReach::sandbox_agent` covers `PlatformOps` and `PlatformApps` on the
platform singleton, and `AppNonProduction` / `AppAdmin` only where the resource names an
environment (`Resource::in_environment`, `EnvFacet`) that is a sandbox the token created,
of an app it is granted — or `EnvFacet::Staging` of an app it was granted **staging** for
(`SandboxApp::staging`, the mint's `staging` option: one more fact, not a fifth action). A
call site that must stay a sandbox's alone — an environment's secrets, deleting one — has
to say so itself, since the model opens staging to such a token. A decision that names no environment is refused for that token
and unchanged for everyone else — which is what lets a call site learn to say which
environment it is about without moving any session's answer.

## Testing

Validation here is **differential, not a shadow window**. At low traffic `disagree == 0`
just means nobody hit it — a false green. The gate is differencing the model against the
legacy oracle, which is what caught every real bug.

| Suite | Proves |
| --- | --- |
| `crates/authz` unit tests (41) | the model's own arithmetic |
| `oxy_server_authz::differential` | the ring agrees with the shipped guard across the caller-shape space |
| `crates/workspace-runtime/tests/integration/authz_loader_differential.rs` | the **real loader** against seeded rows (needs `OXY_DATABASE_URL`; skips without) |
| `crates/guards/tests/integration/authz_boundaries.rs` | nothing outside the allowlist decides access by hand |

The unit differential hand-builds facts, so it tests an *assumption* about the loader; the
seeded suite is what tests the loader. Don't drop a fail-safe on the strength of the
former alone.

## Pitfalls

- **Operator flags are not unconditional.** Global standing must not out-rank a *real*
  membership — an Oxy staffer who is a plain member of a tenant is a plain member there.
  The operator terms are gated on non-membership.
- **`develop_apps` is not `manage_apps`.** Data-plane access vs app lifecycle. Conflating
  them hands a partner another tenant's data.
- **Self rules must be scoped to kind AND action**, or the owner of any future
  owner-bearing resource inherits every action on it.
- **Capability must come from the partner being acted as.** Holding a capability through
  partner B must not authorize anything while scoped to A. `PartnerStanding` keeps the
  partner rather than flattening the sets, which is exactly what makes this expressible.
- **Facts load at most once per request** (memoized in request extensions). On a hot path
  use `load_principal_facts_scoped` so a ring doesn't pay for facts it never reads.
