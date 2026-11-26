import { AlertTriangle, HelpCircle, Info } from "lucide-react";
import type { AirwayPolicyPreviewResponse } from "@/services/api/airwayConfig";
import { splitPipelineRef } from "../../utils";
import { VerdictGroup } from "./VerdictGroup";

/**
 * The fetched preview, grouped by verdict. `not_fixable_here` is surfaced
 * ONCE here, above the failing list — never repeated per row, since several
 * rows can carry it at once and repeating the explanation would bury the
 * signal it exists to send.
 *
 * **What raises the flag changed at airway 0.1.24.** It used to mean "this
 * source kind has no way to declare a contract", which was true of `rest_api`
 * until #105 added a per-endpoint `contract` field to `EndpointConfig`. Every
 * kind can now declare, so the only remaining cause is an *orphaned* contract: the
 * connector declares a contract under a name that is not one of its resources.
 * That is a typo in connector source — no setting on this page, and nothing in
 * the pipeline's YAML, reaches it. The copy below must describe that, not the
 * missing slot.
 */
export function PreviewResults({
  data,
  sourceKind
}: {
  data: AirwayPolicyPreviewResponse;
  sourceKind: string;
}) {
  const failing = data.resources.filter((r) => !r.passes);
  const passing = data.resources.filter((r) => r.passes);
  const notFixable = failing.filter((r) => r.not_fixable_here);
  // The banner used to say "will halt every <kind> pipeline" whenever ANY
  // failing resource was `not_fixable_here`. That is true only when the
  // upstream gap covers every pipeline scanned — and alarmingly false for the
  // partial case, where one connector's orphaned declaration got described as
  // a fleet-wide halt. Count the pipelines actually affected, and say that
  // number. Narrowing the flag to orphaned contracts at 0.1.24 made the
  // partial case the *common* one: an orphan is one name in one connector,
  // where the old missing-slot cause really did cover a whole kind.
  //
  // Keyed on the whole `pipeline_ref` (`{workspace_id}:{path}`), not the path:
  // this is a cross-tenant scan and the same path exists in many workspaces.
  const affected = new Set(notFixable.map((r) => r.pipeline_ref));
  // Both lists, not just `resources`. A pipeline whose connector could not be
  // built produces no resources at all, so counting only `resources` made the
  // denominator smaller than the number of pipelines actually scanned — and
  // "1 of 1 scanned pipelines" read as a fleet-wide halt when a second pipeline
  // had simply landed in `unevaluated`. It IS scanned, just not scored.
  //
  // Still a floor, not an exact count: a pipeline that builds but exposes no
  // resources appears in neither list and has no ref to count. Nothing in the
  // response names it, so the honest options are this floor or a new server
  // field; the floor is only ever wrong in the direction of a smaller
  // denominator on a connector shape that does not exist today.
  const scanned = new Set([
    ...data.resources.map((r) => r.pipeline_ref),
    ...data.unevaluated.map((u) => u.pipeline_ref)
  ]);
  const haltsEveryScannedPipeline = affected.size > 0 && affected.size === scanned.size;

  // Nothing in scope was scored — either the fence is why, or there really are
  // no pipelines of this kind. The two cases read very differently to an
  // operator, and when the fence is the reason it is the SAME idea as the
  // out-of-scope note below: rendering both gave "Not scored: N compiled
  // pipelines…" directly above "No compiled <kind> pipelines in the workspaces
  // your platform scope reaches." Both true, one idea, so they are one
  // sentence.
  const scoredNothing = data.resources.length === 0 && data.unevaluated.length === 0;
  const withheld = data.out_of_scope_pipelines;
  const one = withheld === 1;
  const outOfScopeNote = scoredNothing
    ? `No compiled ${sourceKind} pipelines in the workspaces your platform scope reaches, and the ${withheld} compiled pipeline${
        one ? "" : "s"
      } in the workspaces it does not reach ${one ? "was" : "were"} not scored. Saving the global ${sourceKind} policy still applies to ${one ? "it" : "them"}.`
    : `Not scored: ${withheld} compiled pipeline${
        one ? "" : "s"
      } in workspaces your platform scope does not reach. Saving the global ${sourceKind} policy still applies to ${one ? "it" : "them"}.`;

  return (
    <div className='space-y-3'>
      {affected.size > 0 && (
        <div
          className='flex gap-2 border border-status-warning-text/40 bg-status-warning-bg px-3 py-2 text-status-warning-text text-xs'
          data-testid={`admin-airway-not-fixable-banner-${sourceKind}`}
        >
          <AlertTriangle className='mt-0.5 size-3.5 shrink-0' />
          <p>
            These name a contract the <span className='font-medium'>{sourceKind}</span> connector
            declares for a resource it does not expose — a typo in connector source, which no
            setting here can fix. Saving this will halt{" "}
            {haltsEveryScannedPipeline ? (
              <>
                every <span className='font-medium'>{sourceKind}</span> pipeline scanned (
                {affected.size})
              </>
            ) : (
              <>
                <span className='font-medium'>
                  {affected.size} of {scanned.size}
                </span>{" "}
                scanned <span className='font-medium'>{sourceKind}</span> pipelines — the ones
                listed under Failing below
              </>
            )}{" "}
            until it is corrected upstream.
          </p>
        </div>
      )}

      {failing.length > 0 && (
        <VerdictGroup title='Failing' count={failing.length} tone='fail' resources={failing} />
      )}

      {passing.length > 0 && (
        <VerdictGroup title='Passing' count={passing.length} tone='pass' resources={passing} />
      )}

      {/* The counts above describe only the tenants this operator's platform
          grant reaches — the scan is fenced, because a pipeline_ref names
          another tenant's workspace id and a real file path. The GLOBAL policy
          they can still save is not fenced, so saying nothing here would show
          a short clean list for a fleet-wide change. Placed beside the verdict
          groups rather than at the foot of the card: it qualifies the numbers
          the operator is reading, and is useless anywhere they are not.

          "pipelines", never "<kind> pipelines" — the server counts every kind,
          since source.kind lives inside the compiled definitions the fence
          keeps out of its hands.

          When nothing in scope was scored this note also carries the empty
          state, because "the fence is why this list is empty" is the same
          sentence — see `outOfScopeNote`. */}
      {withheld > 0 && (
        <p
          className='flex gap-2 border border-border/60 px-2.5 py-1.5 text-muted-foreground text-xs'
          data-testid={`admin-airway-out-of-scope-${sourceKind}`}
        >
          <Info className='mt-0.5 size-3.5 shrink-0' />
          <span>{outOfScopeNote}</span>
        </p>
      )}

      {/* Only when the fence is NOT the explanation — otherwise the note above
          already said it. "None found" would be a lie whenever anything was
          withheld: the scope is why the list is empty, not the absence of
          pipelines. */}
      {scoredNothing && withheld === 0 && (
        <p className='text-muted-foreground text-xs'>
          {`No compiled ${sourceKind} pipelines found.`}
        </p>
      )}

      {/* Reported, never gated on. Hiding it once let an operator believe
          coverage was complete; gating on it made every save confirm, since
          some workspace has always never compiled. So it renders as a plain
          scope note rather than a warning — it is a statement about
          workspaces outside this answer, not a gap inside it. */}
      {data.uncompiled_workspaces > 0 && (
        <p
          className='text-muted-foreground text-xs'
          data-testid={`admin-airway-uncompiled-workspaces-${sourceKind}`}
        >
          Scope: {data.uncompiled_workspaces} workspace
          {data.uncompiled_workspaces === 1 ? " has" : "s have"} never compiled a revision, so{" "}
          {data.uncompiled_workspaces === 1 ? "it has" : "they have"} no {sourceKind} pipelines to
          check.
        </p>
      )}

      {/* Neither passing nor failing — rendered whenever present, per the
          requirement that hiding them would let an operator flip a policy
          believing coverage was complete. */}
      {data.unevaluated.length > 0 && (
        <div data-testid={`admin-airway-unevaluated-${sourceKind}`}>
          <div className='mb-1.5 flex items-center gap-1.5 text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
            <HelpCircle className='size-3' /> Unevaluated ({data.unevaluated.length})
          </div>
          <p className='mb-1.5 text-muted-foreground text-xs'>
            Connector could not be built for these pipelines — not scored either way.
          </p>
          <ul className='space-y-1'>
            {data.unevaluated.map((u) => {
              const { path, workspaceId } = splitPipelineRef(u.pipeline_ref);
              return (
                <li key={u.pipeline_ref} className=' border border-border/60 px-2.5 py-1.5 text-xs'>
                  <div className='font-mono'>{path}</div>
                  <div className='mt-0.5 flex items-center justify-between gap-2 text-muted-foreground'>
                    <span className='truncate'>{u.error}</span>
                    <span className='shrink-0 font-mono text-[10px]'>{workspaceId}</span>
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </div>
  );
}
