import { Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useDeleteAirwayWorkspaceOverride } from "@/hooks/api/airwayConfig/useUpsertAirwayConfig";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import { relativeTime } from "@/pages/admin/utils";
import type { AirwayConfigValues, AirwayWorkspaceOverride } from "@/services/api/airwayConfig";
import { AddOverrideDialog } from "./components/AddOverrideDialog";
import { RemoveOverrideDialog } from "./components/RemoveOverrideDialog";

/**
 * Per-workspace overrides for one source kind. Overrides start empty and
 * usually stay that way — an explicit empty state says so, coexisting with
 * (not replaced by) the "Add override" affordance, since a workspace still
 * needs a way to get its first row without hand-writing SQL.
 *
 * **Both write paths confirm.** Adding an override goes through the preview +
 * `computeSaveGate` + `SaveConfirmDialog` chain; removing one goes through
 * `RemoveOverrideDialog`. Removal used to fire on click: it drops the
 * workspace back onto the global row, which may be *stricter* than the
 * override it replaces — the same outage the save gate exists to prevent,
 * reached by a different button.
 *
 * `global` is threaded in from `SourceKindRow` and goes to **both** dialogs:
 * `RemoveOverrideDialog` names the policy taking over rather than describing
 * the fallback in the abstract, and `AddOverrideDialog` needs it to resolve
 * what an "inherit" field will actually run under — a preview computed without
 * it scores airway's default for a workspace that will run the global row's
 * policy. See `resolveInherited`.
 */
export function WorkspaceOverrides({
  sourceKind,
  overrides,
  global
}: {
  sourceKind: string;
  overrides: AirwayWorkspaceOverride[];
  global: AirwayConfigValues | null;
}) {
  const deleteOverride = useDeleteAirwayWorkspaceOverride();
  const [addOpen, setAddOpen] = useState(false);
  const [pendingRemoval, setPendingRemoval] = useState<AirwayWorkspaceOverride | null>(null);

  function confirmRemoval() {
    if (!pendingRemoval) return;
    deleteOverride.mutate(
      { sourceKind, workspaceId: pendingRemoval.workspace_id },
      { onSuccess: () => setPendingRemoval(null) }
    );
  }

  return (
    <div data-testid={`admin-airway-overrides-${sourceKind}`}>
      <div className='mb-1.5 flex items-center justify-between gap-2'>
        <span className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
          Workspace overrides
        </span>
        <Button
          type='button'
          variant='outline'
          size='sm'
          className='h-6 gap-1 px-2 text-[11px]'
          onClick={() => setAddOpen(true)}
          data-testid={`admin-airway-add-override-${sourceKind}`}
        >
          <Plus className='size-3' /> Add override
        </Button>
      </div>

      {overrides.length === 0 ? (
        <p
          className='py-1 text-muted-foreground text-xs'
          data-testid={`admin-airway-overrides-empty-${sourceKind}`}
        >
          No overrides. Every workspace follows the policy above.
        </p>
      ) : (
        <Table>
          <TableHeader>
            <TableRow className={ADMIN_HEADER_ROW_CLASS}>
              <AdminTh>Workspace</AdminTh>
              <AdminTh>Contract policy</AdminTh>
              <AdminTh>Environment</AdminTh>
              <AdminTh>Updated</AdminTh>
              <AdminTh align='right'>Remove</AdminTh>
            </TableRow>
          </TableHeader>
          <TableBody>
            {overrides.map((o) => (
              <TableRow key={o.workspace_id} className='border-border/60'>
                <TableCell className='text-xs'>
                  {o.workspace_name ?? (
                    <span className='font-mono text-muted-foreground'>{o.workspace_id}</span>
                  )}
                </TableCell>
                <TableCell className='text-xs'>
                  {o.values.contract_policy ? (
                    <Badge variant='outline'>{o.values.contract_policy}</Badge>
                  ) : (
                    <span className='text-muted-foreground'>Inherits</span>
                  )}
                </TableCell>
                <TableCell className='text-xs'>
                  {o.values.environment ? (
                    <Badge variant='outline'>{o.values.environment}</Badge>
                  ) : (
                    <span className='text-muted-foreground'>Inherits</span>
                  )}
                </TableCell>
                <TableCell
                  className='text-muted-foreground text-xs tabular-nums'
                  title={new Date(o.values.updated_at).toLocaleString()}
                >
                  {relativeTime(o.values.updated_at)}
                </TableCell>
                <TableCell className='text-right'>
                  <Button
                    type='button'
                    variant='ghost'
                    size='icon'
                    className='size-7 text-muted-foreground hover:text-destructive'
                    disabled={deleteOverride.isPending}
                    onClick={() => setPendingRemoval(o)}
                    aria-label={`Remove ${o.workspace_name ?? o.workspace_id}'s override`}
                    data-testid={`admin-airway-override-remove-${sourceKind}-${o.workspace_id}`}
                  >
                    {deleteOverride.isPending && pendingRemoval?.workspace_id === o.workspace_id ? (
                      <Spinner className='size-3.5' />
                    ) : (
                      <Trash2 className='size-3.5' />
                    )}
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}

      <AddOverrideDialog
        sourceKind={sourceKind}
        open={addOpen}
        onOpenChange={setAddOpen}
        existingWorkspaceIds={overrides.map((o) => o.workspace_id)}
        global={global}
      />

      <RemoveOverrideDialog
        sourceKind={sourceKind}
        override={pendingRemoval}
        global={global}
        pending={deleteOverride.isPending}
        onOpenChange={() => setPendingRemoval(null)}
        onConfirm={confirmRemoval}
      />
    </div>
  );
}
