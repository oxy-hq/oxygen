import type { UseQueryResult } from "@tanstack/react-query";
import type {
  AirwayContractPolicy,
  AirwayEnvironment,
  AirwayPolicyPreviewResponse
} from "@/services/api/airwayConfig";

/**
 * The Save confirm gate, derived from a preview query's state — never from
 * its `data` alone. `data` is `undefined` in situations that are NOT "no
 * failures": before the operator has ever asked for a preview, and whenever
 * the fetch errors. Both used to fall through `?? 0` straight to an
 * unconfirmed save, which is exactly the outage this page exists to prevent
 * (e.g. `require_declared` on `rest_api` halting all ~24 pipelines with zero
 * confirmation). Absence of data means "unknown", not "zero" — this type
 * makes that a case the caller must handle, not a number that quietly means
 * two different things.
 *
 * Every state this has been through, and why each is here:
 *
 * - **`never-previewed`** covers a genuinely un-asked preview *and* a closed
 *   disclosure. React Query serves cached data for an unchanged key even with
 *   `enabled: false`, so `isSuccess` can be `true` off a stale cache hit; a
 *   preview the operator is not looking at is not a preview.
 * - **`incomplete`** is a real coverage gap: `resources` can be `[]` (so
 *   `failingCount === 0`) while every pipeline of the kind sits in
 *   `unevaluated` — a bad definition, a required field an airway bump just
 *   added, a connector the environment refuses. Coverage being incomplete is
 *   not the same fact as coverage being clean.
 *
 *   It keys on `unevaluated` **only**. It must not key on
 *   `uncompiled_workspaces`, which the server reports separately for exactly
 *   this reason: on any real deployment at least one workspace has never
 *   compiled, so a gate that counted those would sit in `incomplete` forever
 *   and confirm every save. A confirmation that always fires is worse than
 *   none — it trains operators to click through the guardrail. The server
 *   still reports the count and the UI still shows it; it is honest to report
 *   and wrong to gate on, because a workspace with nothing compiled has no
 *   pipelines of this kind to check.
 *
 * - **`partial-scope`** is `out_of_scope_pipelines`, and it splits on the
 *   **write tier** — which is why it is not simply excluded alongside
 *   `uncompiled_workspaces`. Those pipelines really were not scored, and the
 *   count is non-zero on *every* request a bounded grant makes: it is a
 *   property of the grant, not of the draft. Gating on it unconditionally
 *   would therefore confirm every save a scoped operator ever performs — the
 *   confirmation-fatigue failure this note already describes once.
 *
 *   But fatigue is the right reading only because the *save* is bounded too,
 *   and that holds for exactly one tier. An **override** lands on one
 *   workspace inside the caller's scope, which the preview did cover, so
 *   `clean` is a true statement about what that save touches. A **global** row
 *   is fleet-wide by construction, and the preview provably did *not* score
 *   `out_of_scope_pipelines` of the pipelines it will govern. A scope-bounded
 *   `global_admin` could flip `toast` to `require_declared`, see a clean scan
 *   of their two orgs, and save with no confirmation for a change that halts
 *   pipelines in tenants they cannot see — "reads as safe, means unknown",
 *   which is the shape this whole gate exists to prevent.
 *
 *   So it fires for the global tier only, and only when the count is non-zero.
 *   That is not a confirmation that always fires: it fires on a *global* policy
 *   flip made by a bounded grant, which is rare, high-stakes, and always
 *   partially blind. Override saves are unaffected, and an unbounded grant
 *   (count `0`) never sees it. It is still surfaced in `PreviewResults` too,
 *   next to the counts it qualifies.
 *
 *   Checked **after** `incomplete`: both mean "impact unknown", and a
 *   `unevaluated` entry names a pipeline the operator can actually go and read,
 *   while this one names nothing they are allowed to see.
 * - **`stale`** is the content check behind the cache key. Every previous fix
 *   here made trust a function of *query state* — `isSuccess`, then
 *   `isSuccess && previewOpen` — and each time a way was found for the state
 *   to be right while the body described different settings. The body now
 *   echoes the `(contract_policy, environment)` it was computed under, so this
 *   compares that against the draft being saved instead of inferring it. The
 *   query key covers both axes, so this should be unreachable; it is here
 *   because "should be unreachable" is what the last two versions of this
 *   function also believed.
 *
 * `environment` is a real admission axis, not cosmetic — `source_factory`
 * refuses connectors under `Sandbox` — so a preview computed under
 * `production` says nothing about a `sandbox` save.
 */
export type SaveGate =
  | { kind: "clean" }
  | { kind: "failures"; failingCount: number }
  | {
      kind: "unknown";
      reason: "never-previewed" | "loading" | "error" | "incomplete" | "partial-scope" | "stale";
      /** Set only when `reason === "incomplete"`. */
      unevaluatedCount?: number;
      /** Set only when `reason === "partial-scope"`. */
      outOfScopeCount?: number;
    };

/**
 * Which row a save writes — the fact that decides whether an unscored
 * remainder is relevant to it.
 *
 * `"global"` writes the kind's `workspace_id IS NULL` row, which governs every
 * tenant's pipelines including the ones a bounded preview could not see.
 * `"override"` writes one workspace's row, and that workspace is inside the
 * caller's scope by construction (the override routes fence on it), so the
 * preview covered everything the save touches.
 *
 * Required rather than defaulted: the two tiers want opposite answers when the
 * remainder is non-zero, so there is no default that is safe for both, and a
 * new call site should have to state which one it is.
 */
export type SaveTier = "global" | "override";

/**
 * The settings the save will actually run under — **already resolved**, not the
 * raw draft. `null` on either field means "nothing applies at any tier", which
 * resolves to airway's default: the same default the server echoes back when
 * the parameter is absent, which is what makes the comparison in
 * {@link computeSaveGate} well-defined.
 *
 * For a workspace override, "resolved" means {@link resolveInherited} has
 * already folded the kind's global row in — an override that leaves a field
 * unset inherits that row, not the built-in default. Passing the raw draft is
 * the defect this type's name exists to prevent.
 *
 * Typed `string`, not the accepted unions, because an inherited value comes
 * from a free-text column and may be a spelling this build doesn't know. Such
 * a value never matches the server's echo (the server refuses to compute a
 * preview for it at all), so the gate lands on `error`/`stale` and confirms —
 * which is the direction to fail in.
 */
export interface PreviewSubject {
  contractPolicy: string | null;
  environment: string | null;
}

/** Airway's defaults, and therefore what the server echoes for an absent parameter. */
const DEFAULT_CONTRACT_POLICY: AirwayContractPolicy = "permissive";
const DEFAULT_ENVIRONMENT: AirwayEnvironment = "production";

/**
 * What a field on an override actually resolves to: the draft if it sets one,
 * otherwise the kind's global row, otherwise airway's built-in default.
 *
 * This is the client-side restatement of `resolve_admission`
 * (`crates/agentic/pipeline/src/airway_config.rs`), which merges field by field
 * with the narrowest non-null value winning. It is not cosmetic: an override
 * that leaves Contract policy on "Inherit this kind's policy" runs under the
 * global row's value, so a preview requested without it scores `permissive` —
 * and because the subject then also defaults to `permissive`, the echo matches
 * and the gate reads **clean**. An all-clear scan renders directly beneath copy
 * promising the workspace follows the global policy, and Save goes through
 * unconfirmed, with `require_declared` never scored.
 *
 * `undefined` is treated as `null`: there is no global row for this kind.
 */
export function resolveInherited(
  draft: string | null,
  globalValue: string | null | undefined
): string | null {
  return draft ?? globalValue ?? null;
}

/** Whether `data` was computed for exactly the settings `subject` describes. */
function describesSubject(data: AirwayPolicyPreviewResponse, subject: PreviewSubject): boolean {
  return (
    data.contract_policy === (subject.contractPolicy ?? DEFAULT_CONTRACT_POLICY) &&
    data.environment === (subject.environment ?? DEFAULT_ENVIRONMENT)
  );
}

/**
 * Shared by the global row (`SourceKindRow`) and the override form
 * (`AddOverrideDialog`) — both save through the same gate.
 *
 * `subject` is the draft about to be saved. A preview is trusted only when the
 * operator has it open **and** its body says it was computed for that exact
 * `(contract_policy, environment)`.
 *
 * `tier` is *where* it will be saved, and it is not cosmetic: a preview fenced
 * to the caller's platform scope answers the whole question for an override and
 * only part of it for the fleet-wide global row. See `partial-scope` on
 * {@link SaveGate}.
 */
export function computeSaveGate(
  preview: UseQueryResult<AirwayPolicyPreviewResponse>,
  previewOpen: boolean,
  subject: PreviewSubject,
  tier: SaveTier
): SaveGate {
  // Cached data for a closed disclosure is not a preview the operator is
  // looking at. Checked first and unconditionally: `preview.isSuccess` can be
  // `true` purely from a stale cache hit, so nothing below this line may run
  // before it.
  if (!previewOpen) return { kind: "unknown", reason: "never-previewed" };

  if (preview.isSuccess) {
    // Before anything derived from the body: a preview of other settings has
    // a failing count, but it is a count for a different question.
    if (!describesSubject(preview.data, subject)) {
      return { kind: "unknown", reason: "stale" };
    }
    const failingCount = preview.data.resources.filter((r) => !r.passes).length;
    if (failingCount > 0) return { kind: "failures", failingCount };
    // `uncompiled_workspaces` is deliberately not consulted here — see the
    // `incomplete` note on `SaveGate`.
    const unevaluatedCount = preview.data.unevaluated.length;
    if (unevaluatedCount > 0) return { kind: "unknown", reason: "incomplete", unevaluatedCount };
    // Last of the "unknown" checks, and only for the fleet-wide row: an
    // override's blast radius sits entirely inside the scan, a global row's
    // provably does not. See the `partial-scope` note on `SaveGate`.
    const outOfScopeCount = preview.data.out_of_scope_pipelines;
    if (tier === "global" && outOfScopeCount > 0) {
      return { kind: "unknown", reason: "partial-scope", outOfScopeCount };
    }
    return { kind: "clean" };
  }
  if (preview.isError) return { kind: "unknown", reason: "error" };
  if (preview.isFetching) return { kind: "unknown", reason: "loading" };
  return { kind: "unknown", reason: "never-previewed" };
}

/**
 * `pipeline_ref` is wire-formatted `"{workspace_id}:{workspace-relative path}"`
 * (see `AirwayResourceVerdict`/`AirwayUnevaluatedPipeline` in
 * `services/api/airwayConfig.ts`). Split on the FIRST `:` — an operator
 * should read the path, not a raw UUID — while keeping the workspace id
 * available as a secondary label, since this is a cross-tenant surface where
 * the same path can exist in many workspaces.
 */
export function splitPipelineRef(ref: string): { workspaceId: string; path: string } {
  const idx = ref.indexOf(":");
  if (idx === -1) return { workspaceId: "", path: ref };
  return { workspaceId: ref.slice(0, idx), path: ref.slice(idx + 1) };
}

/** Shared "Updated <local time>" / "Never configured" label for a config row's freshness. */
export function formatUpdatedAt(value: string | undefined | null): string {
  if (!value) return "Never configured";
  return `Updated ${new Date(value).toLocaleString()}`;
}
