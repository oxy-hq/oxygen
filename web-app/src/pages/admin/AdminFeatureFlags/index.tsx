import { Flag } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { Card, CardContent } from "@/components/ui/shadcn/card";
import { Table, TableBody, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useFeatureFlags, useUpdateFeatureFlag } from "@/hooks/api/featureFlags";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { FlagRow } from "./components/FlagRow";
import { PreviewCard } from "./PreviewCard";
import { RulesDialog } from "./RulesDialog";

type PendingToggle = { key: string; nextValue: boolean };

export default function AdminFeatureFlags() {
  // Deliberately the whole query, not `data: flags = []`: that default made a failed
  // fetch render as "No feature flags defined." — a server that is down and a server
  // with nothing to report looked identical, on a page whose switches change behaviour
  // for every organization.
  const flags = useFeatureFlags();
  const updateFlag = useUpdateFeatureFlag();
  const [pending, setPending] = useState<PendingToggle | null>(null);
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const editing = flags.data?.find((f) => f.key === editingKey);

  const confirmToggle = () => {
    if (!pending) return;
    const { key, nextValue } = pending;
    updateFlag.mutate(
      { key, enabled: nextValue },
      {
        onSuccess: (updated) => {
          toast.success(`${updated.key} is now ${updated.enabled ? "on" : "off"}.`);
        },
        onError: (err) => {
          const message = err instanceof Error ? err.message : "Failed to update flag.";
          toast.error(message);
        },
        onSettled: () => {
          setPending(null);
        }
      }
    );
  };

  return (
    <AdminPage
      width='wide'
      description='Turn flags on or off, and choose which orgs get what. Changes apply immediately on this server.'
      data-testid='admin-feature-flags'
    >
      <AdminAsync
        query={flags}
        noun='feature flags'
        rows={5}
        isEmpty={(rows) => rows.length === 0}
        empty={
          <AdminEmptyState
            icon={Flag}
            title='No feature flags defined.'
            description='Flags are declared in the backend; none are registered on this build.'
          />
        }
      >
        {(rows) => (
          <Card>
            <CardContent className='p-0'>
              <Table>
                <TableHeader>
                  <TableRow className={ADMIN_HEADER_ROW_CLASS}>
                    <AdminTh>Flag</AdminTh>
                    <AdminTh>Serving</AdminTh>
                    <AdminTh className='hidden sm:table-cell'>Changed</AdminTh>
                    <AdminTh align='right'>On</AdminTh>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((flag) => (
                    <FlagRow
                      key={flag.key}
                      flag={flag}
                      switchDisabled={updateFlag.isPending}
                      onToggle={(next) => setPending({ key: flag.key, nextValue: next })}
                      onEditRules={() => setEditingKey(flag.key)}
                    />
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        )}
      </AdminAsync>

      <PreviewCard />

      {editing && (
        <RulesDialog key={editing.key} flag={editing} onClose={() => setEditingKey(null)} />
      )}

      <AlertDialog
        open={pending !== null}
        onOpenChange={(open) => {
          if (!open && !updateFlag.isPending) setPending(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Turn {pending?.nextValue ? "on" : "off"} {pending?.key}?
            </AlertDialogTitle>
            <AlertDialogDescription>
              {pending?.nextValue
                ? "Orgs get their rules again."
                : "Every org gets the off variant, whatever the rules say."}{" "}
              This applies immediately on this server.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel
              disabled={updateFlag.isPending}
              data-testid='admin-feature-flags-cancel'
            >
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              data-testid='admin-feature-flags-confirm'
              disabled={updateFlag.isPending}
              onClick={(event) => {
                event.preventDefault();
                confirmToggle();
              }}
            >
              {updateFlag.isPending ? "Saving…" : "Confirm"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </AdminPage>
  );
}
