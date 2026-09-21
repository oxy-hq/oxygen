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
import { Badge } from "@/components/ui/shadcn/badge";
import { Card, CardContent } from "@/components/ui/shadcn/card";
import { Switch } from "@/components/ui/shadcn/switch";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useFeatureFlags, useUpdateFeatureFlag } from "@/hooks/api/featureFlags";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";

type PendingToggle = { key: string; nextValue: boolean };

function formatUpdatedAt(value: string | null): string {
  if (!value) return "—";
  return new Date(value).toLocaleString();
}

export default function AdminFeatureFlags() {
  // Deliberately the whole query, not `data: flags = []`: that default made a failed
  // fetch render as "No feature flags defined." — a server that is down and a server
  // with nothing to report looked identical, on a page whose switches change behaviour
  // for every organization.
  const flags = useFeatureFlags();
  const updateFlag = useUpdateFeatureFlag();
  const [pending, setPending] = useState<PendingToggle | null>(null);

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
      width='default'
      description='Toggle backend feature flags. Changes apply immediately on this server.'
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
                    <AdminTh>Description</AdminTh>
                    <AdminTh>Default</AdminTh>
                    <AdminTh>Updated</AdminTh>
                    <AdminTh align='right'>Enabled</AdminTh>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((flag) => (
                    <TableRow
                      key={flag.key}
                      className='border-border/60 transition-colors hover:bg-muted/40'
                      data-testid={`admin-feature-flags-row-${flag.key}`}
                    >
                      <TableCell>
                        <span className='font-mono text-xs'>{flag.key}</span>
                      </TableCell>
                      <TableCell className='whitespace-normal break-words text-muted-foreground text-xs'>
                        {flag.description}
                      </TableCell>
                      <TableCell>
                        <Badge variant={flag.default ? "default" : "outline"}>
                          {flag.default ? "On" : "Off"}
                        </Badge>
                      </TableCell>
                      <TableCell className='text-muted-foreground text-xs tabular-nums'>
                        {formatUpdatedAt(flag.updated_at)}
                      </TableCell>
                      <TableCell className='text-right'>
                        <Switch
                          checked={flag.enabled}
                          onCheckedChange={(next) => setPending({ key: flag.key, nextValue: next })}
                          disabled={updateFlag.isPending}
                        />
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        )}
      </AdminAsync>

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
              This change applies immediately on this server and affects all organizations.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={updateFlag.isPending}>Cancel</AlertDialogCancel>
            <AlertDialogAction
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
