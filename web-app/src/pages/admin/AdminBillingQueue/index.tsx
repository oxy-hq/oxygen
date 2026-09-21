import { Inbox, RefreshCw } from "lucide-react";
import { useState } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Card, CardContent, CardHeader } from "@/components/ui/shadcn/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Spinner } from "@/components/ui/shadcn/spinner";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { useAuth } from "@/contexts/AuthContext";
import { useAdminOrgs } from "@/hooks/api/billing";
import type { AdminOrgRow, BillingStatusId } from "@/services/api/billing";
import { AdminAsync } from "../components/AdminAsync";
import { AdminPage } from "../components/AdminPage";
import { BillingDisabledNotice } from "./components/BillingDisabledNotice";
import { OrgAvatar } from "./components/OrgAvatar";
import ProvisionSubscriptionDialog from "./components/ProvisionSubscriptionDialog";
import SubscriptionDetailDialog from "./components/SubscriptionDetailDialog";

const STATUS_OPTIONS: BillingStatusId[] = [
  "incomplete",
  "active",
  "past_due",
  "unpaid",
  "canceled"
];

const STATUS_VARIANT: Record<BillingStatusId, "default" | "secondary" | "destructive" | "outline"> =
  {
    incomplete: "secondary",
    active: "default",
    past_due: "destructive",
    unpaid: "destructive",
    canceled: "outline"
  };

function StatusBadge({ status }: { status: BillingStatusId }) {
  return (
    <Badge variant={STATUS_VARIANT[status]} className='gap-1.5'>
      <span className='size-1.5 rounded-full bg-current opacity-70' />
      {status}
    </Badge>
  );
}

export default function AdminBillingQueue() {
  const [status, setStatus] = useState<BillingStatusId>("incomplete");
  const { authConfig } = useAuth();
  const billingEnabled = authConfig.billing_enabled;
  // Deliberately the whole query, not `data: orgs = []`: that default made a
  // failed fetch render as "No orgs in this status." — a Stripe listing that
  // errored and a status with genuinely nothing in it looked identical, on the
  // queue an operator works to decide whether a tenant has been provisioned.
  const orgs = useAdminOrgs(status, billingEnabled);
  const [selected, setSelected] = useState<AdminOrgRow | null>(null);
  const [detailOrg, setDetailOrg] = useState<AdminOrgRow | null>(null);

  return (
    <AdminPage
      width='default'
      description='Review organizations and provision Stripe subscriptions.'
      actions={
        billingEnabled ? (
          <Button
            variant='outline'
            size='sm'
            onClick={() => orgs.refetch()}
            disabled={orgs.isFetching}
            data-testid='admin-billing-queue-refresh'
          >
            <RefreshCw className={`size-4 ${orgs.isFetching ? "animate-spin" : ""}`} />
            Refresh
          </Button>
        ) : null
      }
      data-testid='admin-billing-queue'
    >
      {!billingEnabled ? (
        <BillingDisabledNotice />
      ) : (
        <Card>
          <CardHeader className='flex-row items-center justify-between gap-2 space-y-0 border-b py-4'>
            <div className='flex items-center gap-3'>
              <span className='text-muted-foreground text-xs'>Status</span>
              {/* Outside the async gate on purpose: the select is what drives
                  the query, so it has to stay usable when that query fails. */}
              <Select value={status} onValueChange={(v) => setStatus(v as BillingStatusId)}>
                <SelectTrigger className='w-40'>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {STATUS_OPTIONS.map((s) => (
                    <SelectItem key={s} value={s}>
                      {s}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {orgs.data ? (
                <span className='text-muted-foreground text-xs'>
                  {orgs.data.length} {orgs.data.length === 1 ? "org" : "orgs"}
                </span>
              ) : null}
            </div>
          </CardHeader>
          <CardContent className='p-0'>
            <AdminAsync
              query={orgs}
              noun='the billing queue'
              // Inset: `CardContent` is `p-0` so the table can reach the card's
              // edges, which would otherwise put the error panel's own border
              // flush against the card's.
              className='m-4'
              skeleton={
                <div className='flex items-center justify-center gap-2 py-16 text-muted-foreground text-xs'>
                  <Spinner /> Loading…
                </div>
              }
              isEmpty={(rows) => rows.length === 0}
              empty={
                <div
                  className='flex flex-col items-center justify-center gap-2 py-16 text-muted-foreground'
                  data-testid='admin-billing-queue-empty'
                >
                  <Inbox className='size-8' />
                  <p className='text-xs'>No orgs in this status.</p>
                </div>
              }
            >
              {(rows) => (
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Org</TableHead>
                      <TableHead>Owner</TableHead>
                      <TableHead>Created</TableHead>
                      <TableHead>Status</TableHead>
                      <TableHead className='text-right'>Actions</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {rows.map((org) => (
                      <TableRow
                        key={org.id}
                        className='hover:bg-muted/40'
                        data-testid={`admin-billing-queue-row-${org.id}`}
                      >
                        <TableCell>
                          <div className='flex items-center gap-3'>
                            <OrgAvatar name={org.name} />
                            <div className='flex flex-col'>
                              <span className='font-medium'>{org.name}</span>
                              <span className='text-muted-foreground text-xs'>/{org.slug}</span>
                            </div>
                          </div>
                        </TableCell>
                        <TableCell className='text-muted-foreground text-xs'>
                          {org.owner_email ?? "—"}
                        </TableCell>
                        <TableCell className='text-muted-foreground text-xs tabular-nums'>
                          {new Date(org.created_at).toLocaleString()}
                        </TableCell>
                        <TableCell>
                          <StatusBadge status={org.status} />
                        </TableCell>
                        <TableCell className='space-x-2 text-right'>
                          {org.status === "incomplete" ? (
                            <Button size='sm' onClick={() => setSelected(org)}>
                              Provision
                            </Button>
                          ) : null}
                          {org.stripe_subscription_id ? (
                            <Button size='sm' variant='outline' onClick={() => setDetailOrg(org)}>
                              Subscription
                            </Button>
                          ) : null}
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </AdminAsync>
          </CardContent>
        </Card>
      )}

      {selected ? (
        <ProvisionSubscriptionDialog
          org={selected}
          onClose={() => setSelected(null)}
          onSuccess={() => {
            setSelected(null);
            orgs.refetch();
          }}
        />
      ) : null}

      {detailOrg ? (
        <SubscriptionDetailDialog org={detailOrg} onClose={() => setDetailOrg(null)} />
      ) : null}
    </AdminPage>
  );
}
