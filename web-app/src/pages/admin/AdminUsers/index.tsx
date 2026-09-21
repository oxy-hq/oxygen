import { RefreshCw, Search, Users } from "lucide-react";
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
import { useAdminUsersList } from "@/hooks/api/adminTenants/useAdminUsers";
import ROUTES from "@/libs/utils/routes";
import type { UserRoleFilter, UserStatusId } from "@/services/api/adminTenants";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { AdminStatusPill } from "../components/AdminStatusPill";
import { ADMIN_HEADER_ROW_CLASS, ADMIN_ROW_CLASS, AdminTh } from "../components/AdminTable";
import { orgRoleKind, platformRoleKind, RoleBadge } from "../components/RoleBadge";

type StatusFilter = "all" | UserStatusId;
/** `staff` = any platform grant; the two role ids narrow to one. */
type RoleFilter = "all" | UserRoleFilter;

/**
 * `/admin/users` — OXY_OWNER-only directory of every user across every
 * organization. Searchable by email/name, filterable by status.
 */
export default function AdminUsers() {
  const navigate = useNavigate();
  const [searchInput, setSearchInput] = useState("");
  const [search, setSearch] = useState("");
  const [status, setStatus] = useState<StatusFilter>("all");
  // Role filter. Narrows server-side (before pagination), so a filtered page is a
  // real page rather than whatever survived a client-side pass over 50 rows.
  const [role, setRole] = useState<RoleFilter>("all");

  // The whole query, not `data: users = []`: that default rendered a failed fetch as
  // "No users yet." — a directory that is down and a deployment with no users looked
  // identical. `AdminAsync` tells the three states apart.
  const users = useAdminUsersList({
    search,
    status: status === "all" ? undefined : status,
    role: role === "all" ? undefined : role
  });
  const { isLoading, isFetching, refetch } = users;

  return (
    <AdminPage
      width='wide'
      description='Every user across the deployment. Filter by role to find who holds staff access, inspect org memberships, and deactivate accounts.'
      data-testid='admin-users'
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
                placeholder='Search by email or name'
                className='pl-8'
              />
            </form>

            <Select value={role} onValueChange={(v) => setRole(v as RoleFilter)}>
              <SelectTrigger className='w-40' data-testid='admin-users-role-filter'>
                <SelectValue placeholder='Any role' />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='all'>Any role</SelectItem>
                {/* "Staff" first: the question this filter exists for is "who can get
                    into the console", and the two roles below narrow it. */}
                <SelectItem value='staff'>Staff (any)</SelectItem>
                <SelectItem value='global_admin'>Global Admin</SelectItem>
                <SelectItem value='app_operator'>App Operator</SelectItem>
              </SelectContent>
            </Select>

            <Select value={status} onValueChange={(v) => setStatus(v as StatusFilter)}>
              <SelectTrigger className='w-36'>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='all'>All statuses</SelectItem>
                <SelectItem value='active'>Active</SelectItem>
                <SelectItem value='deleted'>Deactivated</SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div className='flex items-center gap-3'>
            {!isLoading && users.data ? (
              <span className='text-muted-foreground text-xs'>
                {users.data.length} {users.data.length === 1 ? "user" : "users"}
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
            query={users}
            noun='users'
            rows={6}
            className='p-4'
            isEmpty={(rows) => rows.length === 0}
            empty={
              <AdminEmptyState
                icon={Users}
                title={search ? `No users match "${search}".` : "No users yet."}
                description={
                  search
                    ? "Try a different search term, or clear the filter."
                    : "Users appear here after they sign in for the first time."
                }
              />
            }
          >
            {(rows) => (
              <Table>
                <TableHeader>
                  <TableRow className={ADMIN_HEADER_ROW_CLASS}>
                    <AdminTh>User</AdminTh>
                    <AdminTh align='right'>Orgs</AdminTh>
                    <AdminTh>Status</AdminTh>
                    <AdminTh>Last login</AdminTh>
                    <AdminTh>Joined</AdminTh>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((u) => (
                    <TableRow
                      key={u.id}
                      className={ADMIN_ROW_CLASS}
                      data-testid={`admin-users-row-${u.id}`}
                      onClick={() => navigate(ROUTES.ADMIN.USER_DETAIL(u.id))}
                    >
                      <TableCell>
                        <div className='flex items-center gap-3'>
                          <div className='flex size-8 items-center justify-center rounded-full bg-muted font-medium text-muted-foreground text-xs uppercase'>
                            {(u.name || u.email).slice(0, 1)}
                          </div>
                          <div className='flex flex-col'>
                            <span className='flex flex-wrap items-center gap-1.5 font-medium'>
                              {u.name || u.email}
                              {/* The three authorities are different in KIND, so they
                                stack rather than collapse into one label. */}
                              {(() => {
                                const kind = platformRoleKind(u.platform_role);
                                return kind ? <RoleBadge kind={kind} /> : null;
                              })()}
                              {u.platform_role && !u.platform_scope_all && (
                                <span className='text-[10px] text-muted-foreground'>
                                  {u.platform_scope_org_count} org
                                  {u.platform_scope_org_count === 1 ? "" : "s"}
                                </span>
                              )}
                              {u.top_org_role && <RoleBadge kind={orgRoleKind(u.top_org_role)} />}
                              {/* Delegated cross-org authority via a partner grant —
                                one operator badge per partner they operate. */}
                              {u.partners.map((p) => (
                                <span key={p.id} title={`Partner access at ${p.name}`}>
                                  <RoleBadge kind='partner_operator' />
                                </span>
                              ))}
                            </span>
                            <span className='text-muted-foreground text-xs'>{u.email}</span>
                          </div>
                        </div>
                      </TableCell>
                      <TableCell className='text-right tabular-nums'>{u.org_count}</TableCell>
                      <TableCell>
                        <AdminStatusPill
                          tone={u.status === "deleted" ? "muted" : "ok"}
                          label={u.status === "deleted" ? "Deactivated" : "Active"}
                        />
                      </TableCell>
                      <TableCell className='text-muted-foreground text-xs tabular-nums'>
                        {new Date(u.last_login_at).toLocaleDateString()}
                      </TableCell>
                      <TableCell className='text-muted-foreground text-xs tabular-nums'>
                        {new Date(u.created_at).toLocaleDateString()}
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
