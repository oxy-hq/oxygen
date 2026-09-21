import { Building2, RefreshCw, Search } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useAdminOrgsList } from "@/hooks/api/adminTenants/useAdminOrgs";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { CopyableId } from "@/pages/admin/components/CopyableId";
import { OrgLogo } from "@/pages/admin/components/OrgLogo";
import PartnerChip from "../AdminTenantsCockpit/components/PartnerChip";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { AdminStatusPill } from "../components/AdminStatusPill";
import { ADMIN_HEADER_ROW_CLASS, ADMIN_ROW_CLASS, AdminTh } from "../components/AdminTable";

/**
 * `/admin/orgs` — operator-grade directory of every organization on this
 * deployment. Compact, scan-first table — name + slug on the left, owner
 * email, member / workspace counts in tabular-nums, created date on the
 * right. Master/detail layout: list with a slide-out sheet on row select.
 *
 * Activity / billing columns will land when the backend exposes
 * `updated_at` and a billing status field on `AdminOrgMeta`.
 */
export default function AdminOrgs() {
  const navigate = useNavigate();
  const [search, setSearch] = useState("");
  const [searchInput, setSearchInput] = useState("");
  // The whole query, not `data: orgs = []`: that default rendered a failed fetch as
  // "No organizations yet", so an outage read as an empty deployment.
  const orgs = useAdminOrgsList({ search });
  const { isLoading, isFetching, refetch } = orgs;

  return (
    <AdminPage
      width='wide'
      description='Every organization on this deployment. Select a row to inspect, rename, or transfer ownership. Use the overview hub for fleet-wide health.'
      data-testid='admin-orgs'
    >
      <div className='overflow-hidden rounded-lg border border-border/60 bg-card'>
        {/* Filter strip */}
        <div className='flex items-center justify-between gap-3 border-border/60 border-b px-4 py-3'>
          <form
            className='relative w-full max-w-sm'
            onSubmit={(e) => {
              e.preventDefault();
              setSearch(searchInput.trim());
            }}
          >
            <Search className='absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground' />
            <Input
              value={searchInput}
              onChange={(e) => setSearchInput(e.target.value)}
              placeholder='Search by name or slug'
              className='h-8 pl-7 text-xs'
            />
          </form>
          <div className='flex items-center gap-3'>
            <span className='font-medium text-[10px] text-muted-foreground uppercase tabular-nums tracking-[0.14em]'>
              {isLoading || !orgs.data
                ? "…"
                : `${orgs.data.length.toLocaleString()} org${orgs.data.length === 1 ? "" : "s"}`}
            </span>
            <Button
              variant='ghost'
              size='sm'
              onClick={() => refetch()}
              disabled={isFetching}
              className='h-8'
            >
              <RefreshCw className={cn("size-3.5", isFetching && "animate-spin")} />
              Refresh
            </Button>
          </div>
        </div>

        {/* Body */}
        <AdminAsync
          query={orgs}
          noun='organizations'
          rows={6}
          className='p-6'
          isEmpty={(rows) => rows.length === 0}
          empty={
            <AdminEmptyState
              icon={Building2}
              title={search ? `No organizations match "${search}".` : "No organizations yet"}
              description={
                search
                  ? "Try a different search term, or clear the filter."
                  : "Organizations are created via the signup flow or by an admin from /admin/orgs."
              }
            />
          }
        >
          {(rows) => (
            <Table>
              <TableHeader>
                <TableRow className={ADMIN_HEADER_ROW_CLASS}>
                  <AdminTh>Organization</AdminTh>
                  <AdminTh>Owner</AdminTh>
                  <AdminTh>Partner</AdminTh>
                  <AdminTh>Status</AdminTh>
                  <AdminTh align='right'>Members</AdminTh>
                  <AdminTh align='right'>Workspaces</AdminTh>
                  <AdminTh>Created</AdminTh>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((org) => (
                  <TableRow
                    key={org.id}
                    className={ADMIN_ROW_CLASS}
                    data-testid={`admin-orgs-row-${org.id}`}
                    onClick={() => navigate(ROUTES.ADMIN.ORG_DETAIL(org.id))}
                  >
                    <TableCell>
                      <div className='flex items-center gap-3'>
                        <OrgLogo orgId={org.id} name={org.name} />
                        <div className='flex min-w-0 flex-col'>
                          <span className='truncate font-medium'>{org.name}</span>
                          <div className='flex items-center gap-1'>
                            <span className='truncate font-mono text-[10px] text-muted-foreground'>
                              /{org.slug}
                            </span>
                            <CopyableId value={org.id} className='text-[10px]' />
                          </div>
                        </div>
                      </div>
                    </TableCell>
                    <TableCell className='font-mono text-[11px] text-muted-foreground'>
                      {org.owner_email ?? "—"}
                    </TableCell>
                    {/* Who ELSE administers this tenant. A partner is a delegated
                      cross-org authority — invisible from owner/member counts. */}
                    <TableCell>
                      {org.partner ? (
                        <PartnerChip name={org.partner.name} size='xs' />
                      ) : (
                        <span className='text-muted-foreground/50 text-xs'>—</span>
                      )}
                    </TableCell>
                    <TableCell>
                      <AdminStatusPill
                        tone={org.member_count > 0 ? "ok" : "muted"}
                        label={org.member_count > 0 ? "Active" : "Empty"}
                      />
                    </TableCell>
                    <TableCell className='text-right font-medium text-xs tabular-nums'>
                      {org.member_count.toLocaleString()}
                    </TableCell>
                    <TableCell className='text-right font-medium text-xs tabular-nums'>
                      {org.workspace_count.toLocaleString()}
                    </TableCell>
                    <TableCell className='text-muted-foreground text-xs tabular-nums'>
                      {new Date(org.created_at).toLocaleDateString(undefined, {
                        year: "numeric",
                        month: "short",
                        day: "numeric"
                      })}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </AdminAsync>
      </div>
    </AdminPage>
  );
}
