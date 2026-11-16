import type React from "react";
import { useMemo, useState } from "react";
import {
  LiftAppSecretDialog,
  type LiftTarget
} from "@/components/settings/secrets/LiftAppSecretDialog";
import { DeleteSecretDialog } from "@/components/settings/secrets/SecretTable/Row/DeleteSecretDialog";
import { EditSecretDialog } from "@/components/settings/secrets/SecretTable/Row/EditSecretDialog";
import { ShareSecretDialog } from "@/components/settings/secrets/ShareSecretDialog";
import { useDeleteSecret } from "@/hooks/api/secrets/useSecretMutations";
import useSecrets from "@/hooks/api/secrets/useSecrets";
import { useSharedSecrets } from "@/hooks/api/secrets/useSharedSecrets";
import type { Secret } from "@/types/secret";
import { SecretDetailDialog } from "./components/SecretDetailDialog";
import { SecretsTable } from "./components/SecretsTable";
import { SecretsToolbar } from "./components/SecretsToolbar";
import { buildRows, countByApp, countByKind, filterRows, inScope, readersOf } from "./rows";
import type { AppRef, SecretsFilter, SecretsScope, UnifiedRow } from "./types";

/**
 * One tab's secrets: the workspace's own, or the ones apps keep for themselves.
 *
 * Both tabs are this component with a different `scope`. They share the rows —
 * one request answers both — and differ in what the second column means, what
 * narrows the list, and which verbs a row offers.
 */
export const UnifiedSecretsTable: React.FC<{
  scope: SecretsScope;
  /** Pre-fill the create dialog with a key and the apps to share it with. */
  onCreateFor?: (name: string, appIds: string[]) => void;
}> = ({ scope, onCreateFor }) => {
  const secretsQuery = useSecrets();
  // Sharing and app names. A failure here is not the table's: rows still render,
  // app-only rows fall back to a short id and no sharing is shown.
  const { data: sharing } = useSharedSecrets();
  const deleteSecretMutation = useDeleteSecret();

  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<SecretsFilter>("all");
  const [appId, setAppId] = useState<string | undefined>(undefined);
  const [detailRow, setDetailRow] = useState<UnifiedRow | null>(null);
  const [editSecret, setEditSecret] = useState<Secret | null>(null);
  const [deleteRow, setDeleteRow] = useState<UnifiedRow | null>(null);
  const [shareRow, setShareRow] = useState<UnifiedRow | null>(null);
  const [liftRow, setLiftRow] = useState<UnifiedRow | null>(null);

  const apps = sharing?.apps ?? [];
  const allRows = useMemo(() => {
    const names = new Map((sharing?.apps ?? []).map((a) => [a.app_id, a.app_name]));
    return buildRows(secretsQuery.data?.secrets ?? [], sharing?.shared ?? [], names);
  }, [secretsQuery.data, sharing]);
  const rows = useMemo(() => inScope(allRows, scope), [allRows, scope]);
  const shown = filterRows(rows, scope === "app" ? { appId } : { filter }, query);

  /** The apps that own at least one secret in this tab, for its filter. */
  const owners = useMemo<AppRef[]>(() => {
    const byId = new Map<string, AppRef>();
    for (const row of rows) if (row.app) byId.set(row.app.id, row.app);
    return [...byId.values()].sort((a, b) => a.name.localeCompare(b.name));
  }, [rows]);

  /** The apps that store their own value of `name` — they read it instead. */
  const ownValueOf = (name: string) =>
    new Set(allRows.flatMap((r) => (r.app && r.name === name ? [r.app.id] : [])));

  /** Whether the workspace already stores a secret under this bare key. */
  const workspaceHas = (name: string) =>
    allRows.some((r) => !r.app && r.name.toLowerCase() === name.toLowerCase());

  const handleDelete = () => {
    if (!deleteRow) return;
    // `mutate`, not `mutateAsync`: nothing awaits this handler, so a rejection
    // would go unhandled. The mutation toasts its own failure, and the dialog
    // stays open to try again.
    deleteSecretMutation.mutate(deleteRow.secretInfo.id, {
      onSuccess: () => setDeleteRow(null)
    });
  };

  const liftTarget: LiftTarget | null =
    liftRow?.app != null
      ? {
          secretId: liftRow.secretInfo.id,
          key: liftRow.name,
          appId: liftRow.app.id,
          appName: liftRow.app.name
        }
      : null;

  return (
    <div className='flex flex-col gap-3'>
      {rows.length > 0 && (
        <SecretsToolbar
          scope={scope}
          query={query}
          onQuery={setQuery}
          filter={filter}
          onFilter={setFilter}
          counts={countByKind(rows)}
          apps={owners}
          appCounts={countByApp(rows)}
          appId={appId}
          onApp={setAppId}
        />
      )}

      <SecretsTable
        scope={scope}
        rows={shown}
        total={rows.length}
        loading={secretsQuery.isLoading}
        error={secretsQuery.error?.message}
        // TanStack refetch settles into the query's error state; it does not reject.
        onRetry={() => void secretsQuery.refetch()}
        onOpen={setDetailRow}
        onShare={setShareRow}
        onEdit={(row) => setEditSecret(row.secretInfo)}
        onDelete={setDeleteRow}
        onLift={setLiftRow}
      />

      <SecretDetailDialog
        row={detailRow}
        open={detailRow !== null}
        onOpenChange={(open) => !open && setDetailRow(null)}
        onShare={(row) => {
          setDetailRow(null);
          setShareRow(row);
        }}
        onLift={(row) => {
          setDetailRow(null);
          setLiftRow(row);
        }}
        onEdit={(secret) => {
          setDetailRow(null);
          setEditSecret(secret);
        }}
        onDelete={(row) => {
          setDetailRow(null);
          setDeleteRow(row);
        }}
      />

      <ShareSecretDialog
        secret={
          shareRow
            ? {
                id: shareRow.secretInfo.id,
                name: shareRow.name,
                appIds: shareRow.sharedWith.map((a) => a.id)
              }
            : null
        }
        apps={apps}
        ownValue={shareRow ? ownValueOf(shareRow.name) : undefined}
        onOpenChange={(open) => !open && setShareRow(null)}
        onCreateFor={onCreateFor}
      />

      <LiftAppSecretDialog
        target={liftTarget}
        apps={apps}
        nameTaken={liftRow ? workspaceHas(liftRow.name) : false}
        onOpenChange={(open) => !open && setLiftRow(null)}
        onDone={() => setLiftRow(null)}
      />

      {editSecret && (
        <EditSecretDialog
          open
          onOpenChange={(open) => !open && setEditSecret(null)}
          secret={editSecret}
          onSecretUpdated={() => setEditSecret(null)}
        />
      )}

      {deleteRow && (
        <DeleteSecretDialog
          open
          onOpenChange={(open) => !open && setDeleteRow(null)}
          secret={deleteRow.secretInfo}
          displayName={deleteRow.name}
          readers={readersOf(deleteRow)}
          onConfirm={handleDelete}
          isDeleting={deleteSecretMutation.isPending}
        />
      )}
    </div>
  );
};
