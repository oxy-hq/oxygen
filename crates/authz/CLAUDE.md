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
| **The facts** | `oxy-app` (`server::authz::loader`) | what is *true of* the principal |

The loader stays in `oxy-app` because it reaches app primitives — org membership, partner
standings, the `app_admins` table. This crate depends on `uuid` + `tracing` and nothing
else, which is what lets the whole model be tested without a database.

**Don't add a DB, HTTP, or `entity` dependency here.** A rule that needs a query is a sign
the fact is missing from `PrincipalFacts`, not that the crate needs a connection.

## Vocabulary

- **`Action`** (34) — the closed vocabulary of things a caller can do. This is what call
  sites name.
- **`Ring`** — the authority level that gates an action. **Private on purpose:** a
  ring is how the model is *stated*, not a menu callers pick from. Public, it would let a
  call site choose its own authority level — the scatter this crate exists to end.
- **`PrincipalFacts`** — the whole input surface. Empty = denied everything (fail closed).
- **`Resource`** — what's being acted on: `org_id`, `kind`, optional `owner`, optional
  acting `partner`.
- **`Cap`** (12) — a capability. **One vocabulary, two tiers:** the first eight are the
  partner ceiling (one-to-one with `PartnerCapability`); `ViewTenants` / `ManagePartners` /
  `OperatePlatform` / `ManagePlatformGrants` are platform-only and have no partner
  analogue, deliberately.
- **`Scope`** — `All` or `Orgs(..)`. Where a grant reaches.
- **`PlatformStanding`** / **`PlatformRole`** — Oxy-staff standing as `(caps × scope)`, and
  the presets (`GlobalAdmin`, `AppOperator`) that name common ones.

Rings, briefly: `Read` · `MemberStrict` · `OrgAdmin` · `OrgAdminStrict` · `OwnerOnly` ·
`OrgAdminOrCreator` · `WorkspaceAdmin` · `WorkspaceAdminStrict` · `WorkspaceEdit` ·
`AppAccess` · `WorkspaceData` · `AppAdmin` · `AppGrant` · `PartnerCap` · `StaffReach` · `PlatformAny` ·
`PlatformCap` · `GlobalOwnerOnly`. `StaffReach` is `PlatformCap` on a *tenant* resource: a staff-only
tool used inside one org (non-production app hosts, workspace previews), so the grant's scope is
consulted.

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

From `oxy-app`, prefer the wrappers in `server::authz`: `enforce_guard` (in a guard,
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
4. Add a case to `server::authz::differential` asserting the ring matches the shipped
   check across every caller shape. Reuse an existing ring rather than inventing one —
   two rings that mean the same thing is how drift restarts.
5. Write its arm in `sandbox_agent::covers`, which will not compile without one. That
   match has no wildcard on purpose, and the arm to write is `false`: a sandbox agent
   token (`oxy_sbx_`) covers four actions, and a new one is not a fifth by default.

## A credential narrows as a fact, never as a ring

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
| `server::authz::differential` | the ring agrees with the shipped guard across the caller-shape space |
| `crates/app/tests/authz/authz_loader_differential.rs` | the **real loader** against seeded rows (needs `OXY_DATABASE_URL`; skips without) |
| `crates/app/tests/authz/authz_boundaries.rs` | nothing outside the allowlist decides access by hand |

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
