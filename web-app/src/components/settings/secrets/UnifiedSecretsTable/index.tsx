import { ChevronRight, Code2 } from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { DeleteSecretDialog } from "@/components/settings/secrets/SecretTable/Row/DeleteSecretDialog";
import { EditSecretDialog } from "@/components/settings/secrets/SecretTable/Row/EditSecretDialog";
import { Badge } from "@/components/ui/shadcn/badge";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { useCustomApps } from "@/hooks/api/customApps/useCustomApps";
import { useDeleteSecret } from "@/hooks/api/secrets/useSecretMutations";
import useSecrets from "@/hooks/api/secrets/useSecrets";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { cn } from "@/libs/shadcn/utils";
import type { Secret } from "@/types/secret";
import TableContentWrapper from "../../components/TableContentWrapper";
import TableWrapper from "../../components/TableWrapper";
import { parseAppSecretName } from "./appSecretName";
import { SecretDetailDialog } from "./components/SecretDetailDialog";
import { SOURCE_CONFIG, type UnifiedRow } from "./types";

/** App id → display name, for turning `apps/<uuid>/KEY` into something readable.
 *  An app the workspace list doesn't cover (an unpublished one) falls back to a
 *  short id — still better than the full uuid inline in the variable column. */
type AppNames = Map<string, string>;

function buildRows(secrets: Secret[], appNames: AppNames): UnifiedRow[] {
  const rows: UnifiedRow[] = [];

  for (const secret of secrets) {
    // An app-scoped secret is stored as `apps/<app_id>/<KEY>`. Show it as the
    // key its app actually reads, attributed to the app — the raw storage name
    // is an implementation detail that made these rows unidentifiable.
    const scoped = parseAppSecretName(secret.name);
    rows.push({
      key: `secret-${secret.id}`,
      name: scoped ? scoped.key : secret.name,
      app: scoped
        ? { id: scoped.appId, name: appNames.get(scoped.appId) ?? shortId(scoped.appId) }
        : undefined,
      source: "secret",
      secretInfo: secret
    });
  }

  // Project-wide secrets first, then each app's own, grouped. An app's keys are
  // a set someone reads together — interleaving them alphabetically with the
  // workspace's own variables is what made the app rows hard to find at all.
  rows.sort((a, b) => {
    const group = (a.app?.name ?? "").localeCompare(b.app?.name ?? "");
    return group !== 0 ? group : a.name.localeCompare(b.name);
  });
  return rows;
}

/** Enough of a uuid to tell two apps apart when neither has a resolved name. */
const shortId = (id: string) => id.slice(0, 8);

export const UnifiedSecretsTable: React.FC = () => {
  const {
    data: secretsResponse,
    isLoading: secretsLoading,
    error: secretsError,
    refetch: refetchSecrets
  } = useSecrets();

  // Names for the `apps/<uuid>/` rows. Failure here is cosmetic — a missing
  // name falls back to a short id — so the table never waits on it or reports it.
  const { project } = useCurrentProjectBranch();
  const { data: apps = [] } = useCustomApps(project.id);
  const appNames = useMemo(() => new Map(apps.map((a) => [a.id, a.name])), [apps]);

  const deleteSecretMutation = useDeleteSecret();

  const [detailRow, setDetailRow] = useState<UnifiedRow | null>(null);
  const [editSecret, setEditSecret] = useState<Secret | null>(null);
  const [deleteSecret, setDeleteSecret] = useState<Secret | null>(null);

  const secrets = secretsResponse?.secrets ?? [];
  const rows = buildRows(secrets, appNames);

  const handleDelete = () => {
    if (!deleteSecret) return;
    // `mutate`, not `mutateAsync`: nothing awaits this handler, so a rejection
    // would go unhandled. The mutation toasts its own failure, and the dialog
    // stays open to try again.
    deleteSecretMutation.mutate(deleteSecret.id, { onSuccess: () => setDeleteSecret(null) });
  };

  const handleRefetch = () => {
    // TanStack refetch settles into the query's error state (shown by the table
    // through `error`); it does not reject.
    void refetchSecrets();
  };

  const openDetail = (row: UnifiedRow) => setDetailRow(row);

  return (
    <>
      {/* Each row is a clickable summary (variable + source); the full value,
          metadata and actions live in the detail dialog. On narrow viewports
          TableWrapper collapses each row into a stacked card. */}
      <TableWrapper>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Variable</TableHead>
              <TableHead>Source</TableHead>
              <TableHead className='w-px' />
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableContentWrapper
              isEmpty={rows.length === 0}
              loading={secretsLoading}
              colSpan={3}
              error={secretsError?.message}
              noFoundTitle='No secrets configured'
              noFoundDescription='Create a secret to get started'
              onRetry={handleRefetch}
            >
              {rows.map((row) => {
                const sourceConfig = SOURCE_CONFIG[row.source];

                return (
                  <TableRow
                    key={row.key}
                    className='group cursor-pointer'
                    tabIndex={0}
                    onClick={() => openDetail(row)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        openDetail(row);
                      }
                    }}
                  >
                    <TableCell data-label='Variable'>
                      <div className='flex items-center gap-2'>
                        <Code2 className='size-3.5 shrink-0 text-muted-foreground/50' />
                        <span className='font-medium font-mono text-sm max-md:whitespace-normal max-md:break-all'>
                          {row.name}
                        </span>
                      </div>
                    </TableCell>

                    <TableCell data-label='Source'>
                      <div className='flex flex-col gap-1'>
                        <Badge
                          variant='outline'
                          className={cn("w-fit font-medium text-[10px]", sourceConfig.className)}
                        >
                          {sourceConfig.label}
                        </Badge>
                        {row.app && (
                          <span
                            className='text-[10px] text-muted-foreground'
                            title={`App secret — read as ctx.env.${row.name}`}
                          >
                            {row.app.name}
                          </span>
                        )}
                      </div>
                    </TableCell>

                    <TableCell className='w-px max-md:hidden'>
                      <ChevronRight className='size-4 text-muted-foreground/40 transition-colors group-hover:text-muted-foreground' />
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableContentWrapper>
          </TableBody>
        </Table>
      </TableWrapper>

      <SecretDetailDialog
        row={detailRow}
        open={detailRow !== null}
        onOpenChange={(open) => !open && setDetailRow(null)}
        onEdit={(secret) => {
          setDetailRow(null);
          setEditSecret(secret);
        }}
        onDelete={(secret) => {
          setDetailRow(null);
          setDeleteSecret(secret);
        }}
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
    </>
  );
};
