import {
  AppWindow,
  Building2,
  FolderOpen,
  Handshake,
  MessageSquare,
  Play,
  Search,
  ShieldAlert,
  User
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { AssumeRoleDialog } from "@/components/admin/AssumeRoleDialog";
import { Button } from "@/components/ui/shadcn/button";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator
} from "@/components/ui/shadcn/command";
import { useExplorerRuns, useExplorerThreads } from "@/hooks/api/adminExplorer";
import { useAdminOrgsList } from "@/hooks/api/adminTenants/useAdminOrgs";
import { useAdminUsersList } from "@/hooks/api/adminTenants/useAdminUsers";
import { useAdminWorkspacesList } from "@/hooks/api/adminTenants/useAdminWorkspaces";
import { ADMIN_APP_PAGE_SIZE, useAdminApps } from "@/hooks/api/customApps/useCustomApps";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import ROUTES from "@/libs/utils/routes";
import { matchesQuery } from "@/pages/admin/AdminCustomApps/appStatus";
import { ADMIN_NAV, ADMIN_NAV_GROUPS, itemReachable } from "../AdminLayout/adminNav";

/**
 * Cmd+K / Ctrl+K universal search across orgs, users, and workspaces.
 * The operator-glue that turns three siloed lists into one navigable
 * graph: type "alice" → see Alice's user row → jump straight to her
 * detail page, no detour through `/admin/users`. Then from that page,
 * Cmd+K → "acme" → straight to Acme's org page.
 *
 * Each result group is bordered by a section divider so the operator can
 * quickly distinguish entity types when results are crowded. The trigger
 * pill is the visible affordance; the dialog opens on click or on Cmd+K.
 */
/**
 * Event name a surface dispatches to open the one admin palette. Exported so the caller
 * and the listener cannot drift on the string.
 */
export const ADMIN_PALETTE_OPEN = "admin-palette:open";

/** Open the admin palette from anywhere, without a second ⌘K binding. */
export const openAdminPalette = () => window.dispatchEvent(new CustomEvent(ADMIN_PALETTE_OPEN));

export const AdminEntitySearch = () => {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  // The org an operator has chosen to step into. Reaching a tenant's own product
  // used to mean Directory → switch type → find the row → open the pane → Act as.
  // Five steps to do the thing operators most often need to do, so it lives here:
  // ⌘K, type three letters, act.
  const [actOn, setActOn] = useState<{ id: string; name: string } | null>(null);
  const navigate = useNavigate();

  // Only the rooms this operator can actually enter, by the same rule the rail applies.
  const { data: user } = useCurrentUser();
  const pages = useMemo(() => {
    const standing = {
      isOwner: user?.is_owner ?? false,
      capabilities: user?.platform_capabilities ?? []
    };
    return ADMIN_NAV.filter((item) => itemReachable(item, standing));
  }, [user]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      const isToggle = (event.key === "k" || event.key === "K") && (event.metaKey || event.ctrlKey);
      if (isToggle) {
        event.preventDefault();
        setOpen((prev) => !prev);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // One palette, one shortcut. A surface that wants to offer its own "jump to…" button
  // dispatches this instead of binding ⌘K a second time — the apps console did, and two
  // dialogs opened stacked on one keypress, each filtering its own half of the results.
  useEffect(() => {
    const onOpenRequest = () => setOpen(true);
    window.addEventListener(ADMIN_PALETTE_OPEN, onOpenRequest);
    return () => window.removeEventListener(ADMIN_PALETTE_OPEN, onOpenRequest);
  }, []);

  // Reset query on close so the next open starts clean.
  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

  // The list endpoints already support a `search` query param. We pass the
  // raw input through and let the backend do the heavy lifting; the
  // CommandDialog's built-in cmdk filter then does a final pass on the
  // returned rows so even a slow round-trip feels responsive.
  const orgsQuery = useAdminOrgsList({ search: query }, { enabled: open });
  const usersQuery = useAdminUsersList({ search: query }, { enabled: open });
  const workspacesQuery = useAdminWorkspacesList({ search: query }, { enabled: open });
  // Custom apps. The registry is small (dozens to low hundreds) and already cached by
  // the apps console, which asks for the same page size — hence the shared constant:
  // `pageSize` is part of the query key, so two call sites disagreeing by a digit would
  // silently make this a second full fetch instead of a cache hit.
  const appsQuery = useAdminApps(ADMIN_APP_PAGE_SIZE, { enabled: open });
  // Walk the rest of the registry while the palette is open, like the fleet list and
  // the access pane do. Without this the group held whatever the shared query key
  // happened to have cached — the whole registry on /admin/apps*, where
  // `useAdminAppRegistry` walks it, but only page 1 anywhere else. This palette is
  // mounted in `AdminTopbar` on EVERY admin page, so from /admin/orgs the exact name of
  // app 101 found nothing: the same failure the group comment below says it fixed,
  // surviving at the page boundary instead of the 6-row slice.
  const { hasNextPage, isFetchingNextPage, fetchNextPage, error: appsError } = appsQuery;
  useEffect(() => {
    // `!appsError` for the same reason the registry hook stops: `hasNextPage` is
    // recomputed from the last successful page, so without it a failing page is retried
    // in a loop for as long as the palette stays open.
    if (open && hasNextPage && !isFetchingNextPage && !appsError) {
      fetchNextPage({ cancelRefetch: false });
    }
  }, [open, hasNextPage, isFetchingNextPage, fetchNextPage, appsError]);
  // Only search threads/runs once the operator has typed something — an
  // unfiltered cross-tenant scan on every palette-open isn't worth the round
  // trip.
  const threadsQuery = useExplorerThreads({ search: query }, { enabled: open && query.length > 1 });
  const runsQuery = useExplorerRuns({ search: query }, { enabled: open && query.length > 1 });

  const orgs = useMemo(() => orgsQuery.data?.slice(0, 6) ?? [], [orgsQuery.data]);
  const users = useMemo(() => usersQuery.data?.slice(0, 6) ?? [], [usersQuery.data]);
  const workspaces = useMemo(() => workspacesQuery.data?.slice(0, 6) ?? [], [workspacesQuery.data]);
  // Filter BEFORE slicing. Every other group here passes `search` to its endpoint and
  // gets back rows already narrowed; `useAdminApps` has no such param, so slicing first
  // handed cmdk the six most-recently-updated apps and nothing else — typing the exact
  // name of a seventh app found nothing, on a palette whose trigger reads "Switch app".
  // `matchesQuery` is the registry's own matcher (name, slug, org, and `org/slug`), so
  // the palette and the fleet's search agree on what counts as a match.
  const apps = useMemo(() => {
    const all = appsQuery.data?.pages.flatMap((p) => p.items) ?? [];
    return (query ? all.filter((a) => matchesQuery(a, query)) : all).slice(0, 6);
  }, [appsQuery.data, query]);
  const threads = useMemo(() => threadsQuery.data?.items.slice(0, 6) ?? [], [threadsQuery.data]);
  const runs = useMemo(() => runsQuery.data?.items.slice(0, 6) ?? [], [runsQuery.data]);

  const go = (path: string) => {
    setOpen(false);
    navigate(path);
  };

  return (
    <>
      {actOn && (
        <AssumeRoleDialog
          open
          onOpenChange={(o) => !o && setActOn(null)}
          org={actOn}
          onStarted={() => setActOn(null)}
        />
      )}
      <Button
        variant='outline'
        size='sm'
        onClick={() => setOpen(true)}
        className='h-8 gap-2 px-2.5 text-muted-foreground'
      >
        <Search className='size-3.5' />
        <span className='hidden text-xs sm:inline'>Search</span>
        <kbd className='ml-1 hidden items-center gap-0.5 rounded border border-border/60 bg-muted/40 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground sm:inline-flex'>
          <span className='text-xs'>⌘</span>K
        </kbd>
      </Button>

      <CommandDialog
        open={open}
        onOpenChange={setOpen}
        title='Admin search'
        description='Jump to a page, or find an org, user, workspace, thread or run across the deployment.'
      >
        <CommandInput
          // Apps are named first because the apps console's own trigger reads
          // "Switch app" and opens *this* palette — a placeholder that listed
          // everything except apps left that button promising a thing the dialog
          // it opened never mentioned.
          placeholder='Go to a page, or search apps, orgs, users, workspaces…'
          value={query}
          onValueChange={setQuery}
        />
        <CommandList>
          <CommandEmpty>
            {query.length === 0
              ? "Start typing to search across the tenant graph."
              : "No matches across pages, orgs, users, or workspaces."}
          </CommandEmpty>

          {/* Pages first, and listed even with an empty query. Navigation is the most
              common reason to open a palette, and until now this one could not do it —
              reaching Compile revisions meant finding it in the rail by eye. Driven by
              the same route map the rail renders, filtered by the same capability rule,
              so it can never offer a room the server will refuse. */}
          {pages.length > 0 ? (
            <>
              <CommandGroup heading='Pages'>
                {pages.map((item) => (
                  <CommandItem
                    key={`page-${item.to}`}
                    value={`page ${item.label} ${ADMIN_NAV_GROUPS[item.group]}`}
                    onSelect={() => go(item.to)}
                  >
                    <item.icon className='size-4 text-muted-foreground' />
                    <span className='flex-1 truncate'>{item.label}</span>
                    <span className='text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
                      {ADMIN_NAV_GROUPS[item.group]}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
              <CommandSeparator />
            </>
          ) : null}

          {apps.length > 0 ? (
            <>
              <CommandGroup heading='Custom apps'>
                {apps.map((a) => (
                  <CommandItem
                    key={`app-${a.id}`}
                    // Name alone does not identify an app — `oxy-starter` exists under
                    // two orgs — so every way of referring to it is searchable.
                    value={`app ${a.name} ${a.org_slug} ${a.slug} ${a.org_slug}/${a.slug}`}
                    onSelect={() => go(`/admin/apps/${a.org_slug}/${a.slug}`)}
                  >
                    <AppWindow className='size-4 text-muted-foreground' />
                    <span className='flex-1 truncate'>{a.name}</span>
                    <span className='font-mono text-[11px] text-muted-foreground'>
                      {a.org_slug}/{a.slug}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
              <CommandSeparator />
            </>
          ) : null}

          {orgs.length > 0 ? (
            <>
              <CommandGroup heading='Organizations'>
                {orgs.map((org) => (
                  <CommandItem
                    key={`org-${org.id}`}
                    value={`org ${org.name} ${org.slug} ${org.owner_email ?? ""}`}
                    onSelect={() => go(ROUTES.ADMIN.ORG_DETAIL(org.id))}
                  >
                    {org.is_partner ? (
                      <Handshake className='size-4 text-primary' />
                    ) : (
                      <Building2 className='size-4 text-muted-foreground' />
                    )}
                    <span className='flex-1 truncate'>{org.name}</span>
                    <span className='font-mono text-[11px] text-muted-foreground'>/{org.slug}</span>
                    <span className='hidden text-muted-foreground text-xs tabular-nums sm:inline'>
                      {org.member_count} mbr · {org.workspace_count} ws
                    </span>
                    {/* Enter opens the admin record; this opens the TENANT — two
                        different destinations, so two explicit affordances rather
                        than one overloaded row. */}
                    <Button
                      variant='ghost'
                      size='sm'
                      className='h-6 gap-1 px-1.5 text-[11px]'
                      onClick={(e) => {
                        e.stopPropagation();
                        setOpen(false);
                        setActOn({ id: org.id, name: org.name });
                      }}
                    >
                      <ShieldAlert className='size-3' />
                      Act as
                    </Button>
                  </CommandItem>
                ))}
              </CommandGroup>
              <CommandSeparator />
            </>
          ) : null}

          {users.length > 0 ? (
            <>
              <CommandGroup heading='Users'>
                {users.map((user) => (
                  <CommandItem
                    key={`user-${user.id}`}
                    value={`user ${user.name} ${user.email}`}
                    onSelect={() => go(ROUTES.ADMIN.USER_DETAIL(user.id))}
                  >
                    <User className='size-4 text-muted-foreground' />
                    <span className='flex-1 truncate'>{user.name || user.email}</span>
                    {user.name ? (
                      <span className='truncate font-mono text-[11px] text-muted-foreground'>
                        {user.email}
                      </span>
                    ) : null}
                    <span className='hidden text-muted-foreground text-xs tabular-nums sm:inline'>
                      {user.org_count} org{user.org_count === 1 ? "" : "s"}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
              <CommandSeparator />
            </>
          ) : null}

          {workspaces.length > 0 ? (
            <CommandGroup heading='Workspaces'>
              {workspaces.map((ws) => (
                <CommandItem
                  key={`ws-${ws.id}`}
                  value={`workspace ${ws.name} ${ws.org_slug ?? ""}`}
                  onSelect={() => go(ROUTES.ADMIN.WORKSPACE_DETAIL(ws.id))}
                >
                  <FolderOpen className='size-4 text-muted-foreground' />
                  <span className='flex-1 truncate'>{ws.name}</span>
                  {ws.org_slug ? (
                    <span className='font-mono text-[11px] text-muted-foreground'>
                      /{ws.org_slug}
                    </span>
                  ) : null}
                  <span className='hidden text-muted-foreground text-xs tabular-nums sm:inline'>
                    {ws.member_count} mbr
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
          ) : null}

          {threads.length > 0 ? (
            <>
              <CommandSeparator />
              <CommandGroup heading='Threads'>
                {threads.map((t) => {
                  const openable = t.org_slug && t.workspace_id;
                  return (
                    <CommandItem
                      key={`thread-${t.id}`}
                      value={`thread ${t.title} ${t.workspace_name ?? ""} ${t.org_name ?? ""}`}
                      disabled={!openable}
                      onSelect={() => {
                        if (openable) {
                          go(
                            ROUTES.ORG(t.org_slug as string)
                              .WORKSPACE(t.workspace_id as string)
                              .THREAD(t.id)
                          );
                        }
                      }}
                    >
                      <MessageSquare className='size-4 text-muted-foreground' />
                      <span className='flex-1 truncate'>{t.title || "(untitled)"}</span>
                      {t.workspace_name ? (
                        <span className='hidden truncate font-mono text-[11px] text-muted-foreground sm:inline'>
                          {t.workspace_name}
                        </span>
                      ) : null}
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            </>
          ) : null}

          {runs.length > 0 ? (
            <>
              <CommandSeparator />
              <CommandGroup heading='Runs'>
                {runs.map((r) => {
                  const openable = r.org_slug && r.workspace_id && r.thread_id;
                  return (
                    <CommandItem
                      key={`run-${r.id}`}
                      value={`run ${r.question_snippet} ${r.workspace_name ?? ""} ${r.org_name ?? ""}`}
                      disabled={!openable}
                      onSelect={() => {
                        if (openable) {
                          go(
                            ROUTES.ORG(r.org_slug as string)
                              .WORKSPACE(r.workspace_id as string)
                              .THREAD(r.thread_id as string)
                          );
                        }
                      }}
                    >
                      <Play className='size-4 text-muted-foreground' />
                      <span className='flex-1 truncate'>
                        {r.question_snippet || "(no question)"}
                      </span>
                      {r.task_status ? (
                        <span className='hidden font-mono text-[11px] text-muted-foreground sm:inline'>
                          {r.task_status}
                        </span>
                      ) : null}
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            </>
          ) : null}
        </CommandList>
      </CommandDialog>
    </>
  );
};
