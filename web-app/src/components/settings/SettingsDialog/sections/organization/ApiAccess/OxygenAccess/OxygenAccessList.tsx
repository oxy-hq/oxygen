import { useState } from "react";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Button } from "@/components/ui/shadcn/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/shadcn/dialog";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { useOxygenAccessList } from "@/hooks/api/oxygenAccess";
import type { Organization } from "@/types/organization";
import type { OxygenAccess } from "@/types/oxygenAccess";
import { ListState } from "../shared/ListState";
import { OxygenAccessApproval } from "./OxygenAccessApproval";

const summary = (app: OxygenAccess) => {
  const pending = app.actions.filter((a) => a.status === "pending").length;
  const approved = app.actions.filter((a) => a.status !== "pending").length;
  const parts = [];
  if (approved) parts.push(`${approved} approved`);
  if (pending) parts.push(`${pending} waiting`);
  return parts.join(", ") || "Nothing asked for";
};

/**
 * Every custom app of the organization that asks to act in Oxygen itself, or
 * holds an approval to. Org admins only. Review opens the same panel the
 * approval page shows.
 */
export function OxygenAccessList({ org }: { org: Organization }) {
  const { data, isPending, error, refetch } = useOxygenAccessList(org.id);
  const [open, setOpen] = useState<OxygenAccess | null>(null);

  return (
    <ListState
      what='apps'
      isPending={isPending}
      error={error}
      isEmpty={(data ?? []).length === 0}
      onRetry={refetch}
      empty={
        <p className='text-sm text-muted-foreground'>
          No app of this organization asks to act in Oxygen. An app asks with an <code>oxygen</code>{" "}
          block in its <code>oxy-app.json</code>.
        </p>
      }
      testId='oxygen-access-list'
    >
      <TableWrapper>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>App</TableHead>
              <TableHead>Actions</TableHead>
              <TableHead>Last used</TableHead>
              <TableHead />
            </TableRow>
          </TableHeader>
          <TableBody>
            {(data ?? []).map((app) => (
              <TableRow key={app.app_id} data-testid='oxygen-access-row'>
                <TableCell>{app.app_name}</TableCell>
                <TableCell>{summary(app)}</TableCell>
                <TableCell>
                  {app.last_used_at ? new Date(app.last_used_at).toLocaleString() : "—"}
                </TableCell>
                <TableCell className='text-right'>
                  <Button size='sm' variant='outline' onClick={() => setOpen(app)}>
                    Review
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </TableWrapper>
      <Dialog open={open !== null} onOpenChange={(next) => !next && setOpen(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Apps</DialogTitle>
          </DialogHeader>
          {open && <OxygenAccessApproval orgId={org.id} appId={open.app_id} />}
        </DialogContent>
      </Dialog>
    </ListState>
  );
}
