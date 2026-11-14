import type React from "react";
import { useMemo, useState } from "react";
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
import { buildRows, countByKind, filterRows } from "./rows";
import type { SecretsFilter, UnifiedRow } from "./types";

/**
 * Every secret of the workspace, with who can read it: the workspace alone, the
 * apps it is shared with, or one app's own. Searchable by secret or by app, and
 * filterable by kind, so "what does Platform Pulse read?" is one search.
 */
export const UnifiedSecretsTable: React.FC = () => {
  const secretsQuery = useSecrets();
  // Sharing and app names. A failure here is not the table's: rows still render,
  // app-only rows fall back to a short id and no sharing is shown.
  const { data: sharing } = useSharedSecrets();
  const deleteSecretMutation = useDeleteSecret();

  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<SecretsFilter>("all");
  const [detailRow, setDetailRow] = useState<UnifiedRow | null>(null);
  const [editSecret, setEditSecret] = useState<Secret | null>(null);
  const [deleteSecret, setDeleteSecret] = useState<Secret | null>(null);
  const [shareRow, setShareRow] = useState<UnifiedRow | null>(null);

  const apps = sharing?.apps ?? [];
  const rows = useMemo(() => {
    const names = new Map((sharing?.apps ?? []).map((a) => [a.app_id, a.app_name]));
    return buildRows(secretsQuery.data?.secrets ?? [], sharing?.shared ?? [], names);
  }, [secretsQuery.data, sharing]);
  const shown = filterRows(rows, filter, query);

  /** The apps that store their own value of `name` — they read it instead. */
  const ownValueOf = (name: string) =>
    new Set(rows.flatMap((r) => (r.app && r.name === name ? [r.app.id] : [])));

  const handleDelete = () => {
    if (!deleteSecret) return;
    // `mutate`, not `mutateAsync`: nothing awaits this handler, so a rejection
    // would go unhandled. The mutation toasts its own failure, and the dialog
    // stays open to try again.
    deleteSecretMutation.mutate(deleteSecret.id, { onSuccess: () => setDeleteSecret(null) });
  };

  return (
    <div className='flex flex-col gap-3'>
      {rows.length > 0 && (
        <SecretsToolbar
          query={query}
          onQuery={setQuery}
          filter={filter}
          onFilter={setFilter}
          counts={countByKind(rows)}
        />
      )}

      <SecretsTable
        rows={shown}
        total={rows.length}
        loading={secretsQuery.isLoading}
        error={secretsQuery.error?.message}
        // TanStack refetch settles into the query's error state; it does not reject.
        onRetry={() => void secretsQuery.refetch()}
        onOpen={setDetailRow}
        onShare={setShareRow}
        onEdit={(row) => setEditSecret(row.secretInfo)}
        onDelete={(row) => setDeleteSecret(row.secretInfo)}
      />

      <SecretDetailDialog
        row={detailRow}
        open={detailRow !== null}
        onOpenChange={(open) => !open && setDetailRow(null)}
        onShare={(row) => {
          setDetailRow(null);
          setShareRow(row);
        }}
        onEdit={(secret) => {
          setDetailRow(null);
          setEditSecret(secret);
        }}
        onDelete={(secret) => {
          setDetailRow(null);
          setDeleteSecret(secret);
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
      />

      {editSecret && (
        <EditSecretDialog
          open
          onOpenChange={(open) => !open && setEditSecret(null)}
          secret={editSecret}
          onSecretUpdated={() => setEditSecret(null)}
        />
      )}

      {deleteSecret && (
        <DeleteSecretDialog
          open
          onOpenChange={(open) => !open && setDeleteSecret(null)}
          secret={deleteSecret}
          onConfirm={handleDelete}
          isDeleting={deleteSecretMutation.isPending}
        />
      )}
    </div>
  );
};
