import { FolderOpen, RefreshCw, Search } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { Card, CardContent, CardHeader } from "@/components/ui/shadcn/card";
import { Input } from "@/components/ui/shadcn/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useAdminWorkspacesList } from "@/hooks/api/adminTenants/useAdminWorkspaces";
import ROUTES from "@/libs/utils/routes";
import { CopyableId } from "@/pages/admin/components/CopyableId";
import type { WorkspaceStatusId } from "@/services/api/adminTenants";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { AdminStatusPill, type AdminStatusTone } from "../components/AdminStatusPill";
import { ADMIN_HEADER_ROW_CLASS, ADMIN_ROW_CLASS, AdminTh } from "../components/AdminTable";

type StatusFilter = "all" | WorkspaceStatusId;

const STATUS_PILL: Record<WorkspaceStatusId, { tone: AdminStatusTone; label: string }> = {
  ready: { tone: "ok", label: "Ready" },
  cloning: { tone: "info", label: "Cloning" },
  failed: { tone: "danger", label: "Failed" },
  not_oxy_project: { tone: "muted", label: "Not Oxy" }
};

/**
 * `/admin/workspaces` — OXY_OWNER-only directory of every workspace
 * across every organization.
 */
export default function AdminWorkspaces() {
  const navigate = useNavigate();
  const [searchInput, setSearchInput] = useState("");
  const [search, setSearch] = useState("");
  const [status, setStatus] = useState<StatusFilter>("all");

  // The whole query, not `data: workspaces = []`: that default rendered a failed fetch
  // as "No workspaces yet.", so an outage and an empty deployment looked the same.
  const workspaces = useAdminWorkspacesList({
    search,
    status: status === "all" ? undefined : status
  });
  const { isLoading, isFetching, refetch } = workspaces;

  return (
    <AdminPage
      width='wide'
      description='Every workspace across every organization. Search, filter by status, and operate on the membership.'
      data-testid='admin-workspaces'
    >
      <Card>
        <CardHeader className='flex-row items-center justify-between gap-2 space-y-0 border-b py-4'>
          <div className='flex flex-1 items-center gap-3'>
            <form
              className='relative w-full max-w-sm'
              onSubmit={(e) => {
                e.preventDefault();
                setSearch(searchInput.trim());
              }}
            >
              <Search className='absolute top-1/2 left-2 size-4 -translate-y-1/2 text-muted-foreground' />
              <Input
                value={searchInput}
                onChange={(e) => setSearchInput(e.target.value)}
                placeholder='Search by workspace name'
                className='pl-8'
              />
            </form>

            <Select value={status} onValueChange={(v) => setStatus(v as StatusFilter)}>
              <SelectTrigger className='w-36'>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='all'>All statuses</SelectItem>
                <SelectItem value='ready'>Ready</SelectItem>
                <SelectItem value='cloning'>Cloning</SelectItem>
                <SelectItem value='failed'>Failed</SelectItem>
                <SelectItem value='not_oxy_project'>Not Oxy project</SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div className='flex items-center gap-3'>
            {!isLoading && workspaces.data ? (
              <span className='text-muted-foreground text-xs'>
                {workspaces.data.length} {workspaces.data.length === 1 ? "workspace" : "workspaces"}
              </span>
            ) : null}
            <Button variant='outline' size='sm' onClick={() => refetch()} disabled={isFetching}>
              <RefreshCw className={`size-4 ${isFetching ? "animate-spin" : ""}`} />
              Refresh
            </Button>
          </div>
        </CardHeader>
        <CardContent className='p-0'>
          <AdminAsync
            query={workspaces}
            noun='workspaces'
            rows={6}
            className='p-4'
            isEmpty={(rows) => rows.length === 0}
            empty={
              <AdminEmptyState
                icon={FolderOpen}
                title={search ? `No workspaces match "${search}".` : "No workspaces yet."}
                description={
                  search
                    ? "Try a different search term, or clear the filter."
                    : "Workspaces appear here as organizations import or create projects."
                }
              />
            }
          >
            {(rows) => (
              <Table>
                <TableHeader>
                  <TableRow className={ADMIN_HEADER_ROW_CLASS}>
                    <AdminTh>Workspace</AdminTh>
                    <AdminTh>Organization</AdminTh>
                    <AdminTh>Status</AdminTh>
                    <AdminTh align='right'>Members</AdminTh>
                    <AdminTh>Last opened</AdminTh>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((w) => (
                    <TableRow
                      key={w.id}
                      className={ADMIN_ROW_CLASS}
                      data-testid={`admin-workspaces-row-${w.id}`}
                      onClick={() => navigate(ROUTES.ADMIN.WORKSPACE_DETAIL(w.id))}
                    >
                      <TableCell>
                        <div className='flex items-center gap-3'>
                          <div className='flex size-8 items-center justify-center rounded-md bg-muted text-muted-foreground'>
                            <FolderOpen className='size-4' />
                          </div>
                          <div className='flex flex-col'>
                            <span className='font-medium'>{w.name}</span>
                            <div className='flex items-center gap-1.5'>
                              <CopyableId value={w.id} className='text-[10px]' />
                              <span className='text-muted-foreground text-xs'>
                                Created {new Date(w.created_at).toLocaleDateString()}
                              </span>
                            </div>
                          </div>
                        </div>
                      </TableCell>
                      <TableCell className='text-muted-foreground text-xs'>
                        {w.org_slug ? `/${w.org_slug}` : <span className='italic'>orphaned</span>}
                      </TableCell>
                      <TableCell>
                        <AdminStatusPill
                          tone={STATUS_PILL[w.status].tone}
                          label={STATUS_PILL[w.status].label}
                        />
                      </TableCell>
                      <TableCell className='text-right tabular-nums'>{w.member_count}</TableCell>
                      <TableCell className='text-muted-foreground text-xs tabular-nums'>
                        {w.last_opened_at ? new Date(w.last_opened_at).toLocaleDateString() : "—"}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </AdminAsync>
        </CardContent>
      </Card>
    </AdminPage>
  );
}
