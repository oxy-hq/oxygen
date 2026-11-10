/**
 * Standing a customer up, as a table rather than as tribal knowledge.
 *
 * ONE DECLARATION PER STEP, and everything else in this directory reads it:
 * the plan, the checklist, the verify pass and the JSON report are four
 * renderings of this array. That is the point — the sequence existed only in
 * people's heads, so a second copy of it is how it drifts again.
 *
 * Three fields carry the weight, and each answers a question a runbook has to
 * answer or it is not a runbook:
 *
 *   `surface`     WHO can do it, and therefore whether a staff caller needs an
 *                 assume-role session. `admin` and `tenant` are mutually
 *                 exclusive in one session: an active assume session CLOSES
 *                 `/admin` (`crates/app/src/server/api/admin/mod.rs` — the
 *                 staff surface is refused while acting as a tenant), so a
 *                 standup is several stages, not one pass. `stages()` below is
 *                 what computes them.
 *   `automation`  whether `oxyc` does it, does part of it, only verifies it, or
 *                 merely reads a state nobody performs. `manual` needs a reason
 *                 in `manualBecause`, and the only reasons accepted are "a
 *                 person must buy a paid resource", "the authority is
 *                 owner-only", "a credential's VALUE has to come from a human"
 *                 and "somebody has to hold the physical device". "Nobody wrote
 *                 it" is not one, which is why there are four `manual` rows and
 *                 not nine.
 *   `idempotent`  what a SECOND run does. `"conflicts"` is the dangerous class:
 *                 the call succeeds and leaves a duplicate. A half-finished
 *                 standup is the common case, so every write `run` performs is
 *                 `true` here and every `"conflicts"` row is read-then-write.
 */

/** The phases, in the order a standup goes through them. */
export const PHASE_IDS = [
  "repo",
  "org",
  "billing",
  "oltp",
  "git-link",
  "workspace",
  "airhouse",
  "app",
  "secrets",
  "frontline"
] as const;

export type PhaseId = (typeof PHASE_IDS)[number];

/**
 * Where the step is performed, which decides what credential it needs.
 *
 * `local`  this machine and GitHub — no Oxy credential at all.
 * `admin`  Oxy staff standing on `/api/admin` or a staff-gated surface.
 * `tenant` inside the customer's org, so staff need `oxyc assume start`.
 */
export type Surface = "local" | "admin" | "tenant";

/** How much of the step `oxyc` performs. */
export type Automation =
  /** `oxyc bootstrap run` does it, and re-running is safe. */
  | "automated"
  /** `oxyc` does part; the rest is printed as a step with a command. */
  | "assisted"
  /** `oxyc` only prints it and verifies afterwards that it was done. */
  | "manual"
  /**
   * Nobody performs it: it is the state the phases before it produce, and the
   * only honest act is to read it. Distinct from `manual` on purpose — a row
   * that says "you" for something there is nothing to do about teaches the
   * reader to skim the column.
   */
  | "verified";

/** What a second run of the step does. */
export type Repeatability =
  /** Re-running reconciles. Safe for `run` to perform unconditionally. */
  | true
  /** Re-running is refused, loudly. Safe, but `run` should read first. */
  | false
  /** Re-running SUCCEEDS and leaves a duplicate. Always read-then-write. */
  | "conflicts";

export interface Phase {
  id: PhaseId;
  /** One line, in the operator's words. */
  title: string;
  surface: Surface;
  automation: Automation;
  /** The endpoint, command or file this step actually IS. */
  how: string;
  /** Which role may do it. */
  who: string;
  idempotent: Repeatability;
  /** What it looks like when it has gone wrong — the symptom, not the cause. */
  wrong: string;
  /** Required on `manual`, and `assertPhaseTable` enforces that. */
  manualBecause?: string;
  /** Phases that must have landed before this one can. */
  needs: PhaseId[];
  /** Skipped unless the customer has frontline crews (`--crews`). */
  crewsOnly?: boolean;
}

export const PHASES: readonly Phase[] = [
  {
    id: "repo",
    title: "the customer's workspace repository exists, with a remote",
    surface: "local",
    automation: "assisted",
    how: '`oxyc new <customer> --display "<Name>"` creates `oxy-hq/<customer>` from the template, tags it with the customer topic and scaffolds it locally; it commits and pushes NOTHING, so a person does the first push',
    who: "an Oxy engineer with `gh` logged in",
    idempotent: false,
    wrong:
      "`oxyc new` refuses because the repo is already there (use `oxyc import`), or the scaffold sits unpushed — which looks finished locally and is invisible to the deployment",
    needs: []
  },
  {
    id: "org",
    title: "the organization, its owner, and the blank `Default` workspace",
    surface: "admin",
    automation: "automated",
    how: "`POST /api/admin/orgs` `{name, slug?, owner_email}` → `{org, owner_status, default_workspace_id}`. The route is **IdeOnly** (`CREATE_ORG_ROLE` in `crates/api-tenancy/src/admin/orgs.rs`) because it scaffolds a working copy, so it is answered by the Factory pod",
    who: "staff holding `Action::PlatformOrgCreate` (`Cap::CreateOrgs`) — NOT an App Operator",
    idempotent: false,
    wrong:
      'a taken or reserved slug is a 4xx; an `owner_email` with no account yet answers 200 with `owner_status: "invited"`, which reads as success while nobody can sign in until the invitation is accepted (7-day expiry)',
    needs: []
  },
  {
    id: "billing",
    title: "a subscription, so the org's own routes stop answering 402",
    surface: "admin",
    automation: "manual",
    how: "`POST /api/admin/orgs/{org_id}/billing/provision-subscription`. A new org is created billing-`Incomplete`, and `Incomplete` does not grant access (`crates/billing/src/state.rs` `grants_access`)",
    who: "the Global Owner only — the billing sub-router escalates to the strict owner guard (`Ring::GlobalOwnerOnly`)",
    idempotent: true,
    wrong:
      'every org-scoped and workspace-scoped route behind `subscription_guard` answers `402 {code: "subscription_required"}` — including the GitHub import that gives a workspace its remote. On a deployment with no Stripe configuration, or with the `billing` flag off, the guard short-circuits and this phase does not apply',
    manualBecause:
      "the authority is owner-only by design, and the act commits the account to a price",
    needs: ["org"]
  },
  {
    id: "oltp",
    title: "the org's OLTP Postgres, its writers, and the shared staging branch",
    surface: "admin",
    automation: "manual",
    how: "`oxyc oltp provision --org <slug> --writer app:<slug> --branch staging` → `POST /api/admin/orgs/{org_id}/oltp/provision`; read back with `GET /api/admin/orgs/{org_id}/oltp` and `GET …/oltp/branches/staging`",
    who: "staff holding `Action::PlatformOltp`",
    idempotent: true,
    wrong:
      "`503` means the deployment has no OLTP provider at all; `409` is provider state a person resolves; a provider quota (a Neon project cap bit production on 2026-10-21) surfaces as a failed provision, not as a refusal. A function whose store is missing fails at runtime naming the operator step",
    manualBecause:
      "it buys a billable project at a paid provider, and `internal-docs/data-placement.md` rules that there is no automatic provisioning. The org's shared staging branch is the same decision; only a custom-app ENVIRONMENT's branch is lifecycle-managed",
    needs: ["org"]
  },
  {
    id: "git-link",
    title: "a workspace that is the customer's repository, not a blank directory",
    surface: "tenant",
    automation: "assisted",
    how: "a git namespace first (`GET /api/orgs/{org_id}/github/namespaces`, created by `POST …/namespaces/installation` after the GitHub App install, or `POST …/namespaces/pat`), then `POST /api/orgs/{org_id}/onboarding/github` `{namespace_id, repo_id, branch, name?, subdir?}`",
    who: "a member of the org — so staff need `oxyc assume start --org <slug>`, which closes `/admin` for the session",
    idempotent: false,
    wrong:
      "the workspace has `git_remote_url: null` — **the only path that records a remote is this import**. It connects the repository to the `Default` workspace every org is created with (same id), so until it runs that workspace has none. An org has one workspace: a second import answers `409 org_has_workspace` instead of creating `Default 2`",
    needs: ["org", "billing"]
  },
  {
    id: "workspace",
    title: "the workspace that serves the customer's code is Ready and compiled",
    surface: "admin",
    automation: "verified",
    how: "`GET /api/admin/workspaces-meta?org_id=<uuid>` lists them; `GET /api/admin/workspaces/{id}/detail` is the only read that carries `git_remote_url` and `current_revision_id`",
    who: "staff holding `Action::PlatformOrgs`",
    idempotent: true,
    wrong:
      "`status` is not Ready, or `current_revision_id` is null — nothing is compiled, so per-request reads answer *retryable* and Workspace Health reports Degraded. A workspace with a remote and no revision is a Factory that has not pulled",
    needs: ["git-link"]
  },
  {
    id: "airhouse",
    title: "the workspace's Airhouse tenant and its service account",
    surface: "admin",
    automation: "automated",
    how: "the server provisions it on workspace creation; `POST /api/admin/workspaces/{workspace_id}/airhouse/provision` (no body) repairs or backfills, and `GET /api/admin/airhouse` is the fleet read",
    who: "staff holding `Action::PlatformAirhouse`",
    idempotent: true,
    wrong:
      '`status: "none"` (no tenant) or `service_account_ready: false` — the workspace cannot mint the credential every query uses, so reads fail rather than return nothing. `409` means the global tenant name is taken and a person has to resolve it',
    needs: ["workspace"]
  },
  {
    id: "app",
    title: "the first app published, promoted, and serving the customer's data",
    surface: "admin",
    automation: "assisted",
    how: "`oxyc publish --org <slug> --app <slug> --project <workspace-uuid>` from the app directory (the first publish needs `--project`, and the org is named by SLUG or the bundle's chunks 404), then `oxyc promote`. The bundle's `migrations` and `airhouseMigrations` are applied by the promote, recorded in `custom_app_migrations`",
    who: "staff with the app-admin role, or the app's own service account over GitHub OIDC (`oxyc init-ci`)",
    idempotent: true,
    wrong:
      "`409 staging_never_served` — production only serves a build staging has served; a build published by a **sandbox agent token** needs a person to republish it before it may go live. An unknown app answers 404, not 401, and a custom-app host answers 200 with the shell for every path, so a 200 proves nothing",
    needs: ["workspace", "oltp"]
  },
  {
    id: "secrets",
    title: "every declared secret has a value in each environment that needs one",
    surface: "admin",
    automation: "manual",
    how: "`GET /api/customer-apps/{id}/secrets?environment=production|staging` reports `missing_required`; `POST …/secrets` `{key, value, environment}` sets one. `oxyc env secret` deliberately reaches sandboxes only, so staging and production values go through the console or `oxyc api`. Workspace-level secrets (LLM keys, warehouse credentials) are a different store: `/api/workspaces/{workspace_id}/secrets`, WorkspaceAdmin",
    who: "an org admin in the console, or staff with the app-admin role over the API",
    idempotent: true,
    wrong:
      "`missing_required > 0`. Values are PER ENVIRONMENT, so an app that works in staging can still be missing a production secret — and the symptom is a function failing closed at runtime, not a publish that refuses",
    manualBecause:
      "the value is a credential a human (or their secret manager) holds. There is no reveal verb anywhere in `oxyc` for the same reason: a value in an agent's context is a value an injected instruction can print",
    needs: ["app"]
  },
  {
    id: "frontline",
    title: "crew identities and the kiosks their PINs work on",
    surface: "tenant",
    automation: "manual",
    how: "`POST /api/orgs/{org_id}/frontline/workers` enrols one; `PUT …/workers/{user_id}/apps` grants app access; `POST …/devices` registers a kiosk and returns the one-time enrol link a person opens ON the tablet (`GET`/`POST /api/frontline/devices/bind`), reissued by `POST …/devices/{id}/enrol-link`",
    who: "an org admin — so staff need an assume-role session",
    idempotent: "conflicts",
    wrong:
      "a PIN is refused exactly as a wrong PIN would be, because the browser is not an enrolled kiosk — the `oxy_kiosk` device cookie is what gates PIN login. A second enrol of the same person creates a second worker",
    manualBecause:
      "binding a kiosk requires someone to open a one-time link on the physical tablet, and a PIN is a credential",
    crewsOnly: true,
    needs: ["billing", "workspace"]
  }
];

/**
 * One run of consecutive phases on the same surface.
 *
 * The boundary between two stages is a CREDENTIAL CHANGE, not a pause for
 * effect: crossing into `tenant` means `oxyc assume start`, and crossing back
 * means `oxyc assume end`, because the two surfaces cannot be held at once.
 * A standup therefore crosses twice — the GitHub import is a tenant act that
 * sits between admin acts — and that is a property of the sequence, not of
 * this renderer.
 */
export interface Stage {
  surface: Surface;
  phases: Phase[];
}

export function stages(phases: readonly Phase[]): Stage[] {
  const out: Stage[] = [];
  for (const phase of phases) {
    const last = out.at(-1);
    if (last && last.surface === phase.surface) last.phases.push(phase);
    else out.push({ surface: phase.surface, phases: [phase] });
  }
  return out;
}

export interface PhaseSelection {
  /** Include the `crewsOnly` phases. */
  crews?: boolean;
  /** Only these ids, in table order. Empty or absent means all of them. */
  only?: PhaseId[];
}

export function isPhaseId(value: string): value is PhaseId {
  return (PHASE_IDS as readonly string[]).includes(value);
}

/** The phases this invocation is about, in table order. */
export function phasesFor(selection: PhaseSelection = {}): Phase[] {
  const only = selection.only ?? [];
  return PHASES.filter(
    (p) =>
      (only.length === 0 || only.includes(p.id)) &&
      (selection.crews === true || p.crewsOnly !== true)
  );
}

export function phase(id: PhaseId): Phase {
  const found = PHASES.find((p) => p.id === id);
  // Unreachable through `PhaseId`; present so the lookup has no `!`.
  if (!found) throw new Error(`no such phase: ${id}`);
  return found;
}

/**
 * The table's own invariants, as a function so a unit test is the enforcement
 * and not a comment.
 *
 * Returns the problems rather than throwing, so the test can report all of
 * them at once instead of the first.
 */
export function assertPhaseTable(): string[] {
  const problems: string[] = [];
  const seen = new Set<PhaseId>();
  for (const p of PHASES) {
    if (seen.has(p.id)) problems.push(`${p.id}: declared twice`);
    seen.add(p.id);
    // A dependency later in the array would make `stages()` wrong and the
    // printed order a lie.
    for (const need of p.needs) {
      if (!seen.has(need)) problems.push(`${p.id}: needs ${need}, which comes later or not at all`);
    }
    if (p.automation === "manual" && !p.manualBecause) {
      problems.push(`${p.id}: manual with no reason — "nobody wrote it" is not one`);
    }
    if (p.automation !== "manual" && p.manualBecause) {
      problems.push(`${p.id}: carries a manual reason but is not manual`);
    }
  }
  if (PHASES.length !== PHASE_IDS.length) {
    problems.push(`PHASE_IDS has ${PHASE_IDS.length} ids for ${PHASES.length} phases`);
  }
  return problems;
}
