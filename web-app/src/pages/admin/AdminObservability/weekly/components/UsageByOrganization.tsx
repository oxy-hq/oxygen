import { ChevronRight } from "lucide-react";
import { Fragment, type ReactNode, useEffect, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { Card, CardContent } from "@/components/ui/shadcn/card";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import type { UsageOrg } from "@/types/usageReport";
import { type UsageColumn, usageColumns } from "../usageColumns";
import { appConsolePath, isQuietOrg } from "../utils";

/**
 * A row's figure in one column — or, where the row has none, the column's own words for
 * that, muted. "not measured" must not look like a number: it is the absence of one.
 */
const figure = (value: string | null, column: UsageColumn): ReactNode =>
  value ?? <span className='font-normal text-muted-foreground'>{column.missing}</span>;

/**
 * Above this many organizations the table folds: one row per organization, its apps
 * behind a disclosure, and the organizations that did nothing all week folded into one
 * line. At or under it every app is listed, as the email does.
 */
export const COMPACT_ABOVE = 10;

/**
 * Every organization in the report and, under each, its apps — one table, so a number
 * can be read down its column across organizations.
 *
 * No colour on the change column. On this surface colour means something is wrong, and
 * the highlights above have already said what is; a fall of two people at a three-person
 * shop is not an alarm.
 *
 * `#org-<id>` lands on one organization: scrolled to, opened, and marked. Accounts Insight
 * and the Monday email link an account here that way. The report loads after the page, so
 * the browser's own anchor jump would find nothing to jump to.
 *
 * Built for a fleet of ~100. Listing every app of every org — the email's shape — made
 * the table a few thousand pixels long at that size, mostly orgs that opened nothing; so
 * past [`COMPACT_ABOVE`] the table keeps the server's order but folds what is quiet.
 */
export function UsageByOrganization({ orgs }: { orgs: readonly UsageOrg[] }) {
  const { shown, hidden } = usageColumns(orgs);
  const { hash } = useLocation();
  // Compared raw: an org id is a UUID, which needs no decoding, and decoding a hand-edited
  // `#org-%E0` would throw during render.
  const target = hash.startsWith("#org-") ? hash.slice("#org-".length) : null;
  const compact = orgs.length > COMPACT_ABOVE;

  const [open, setOpen] = useState<Set<string>>(() => new Set(target ? [target] : []));
  const [showQuiet, setShowQuiet] = useState(false);
  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  // The linked org is never folded away, quiet or not.
  const quiet = compact ? orgs.filter((o) => isQuietOrg(o) && o.org_id !== target) : [];
  const listed = compact && !showQuiet ? orgs.filter((o) => !quiet.includes(o)) : orgs;

  useEffect(() => {
    if (!target || !orgs.some((o) => o.org_id === target)) return;
    document.getElementById(`org-${target}`)?.scrollIntoView?.({ block: "center" });
  }, [target, orgs]);

  return (
    <section className='space-y-2' data-testid='admin-usage-report-orgs'>
      <div className='space-y-1'>
        <h3 className='font-semibold text-sm'>By organization</h3>
        <p className='text-muted-foreground text-xs'>
          Figures are for the week of the report. Change is the number of people compared with the
          week before.{compact ? " Open an organization to see its apps." : null}
        </p>
      </div>

      {orgs.length === 0 ? (
        <p className='text-muted-foreground text-xs' data-testid='admin-usage-report-orgs-empty'>
          No organizations are in this report.
        </p>
      ) : (
        <Card className='overflow-hidden'>
          <CardContent className='p-0'>
            <Table>
              <TableHeader>
                <TableRow className={ADMIN_HEADER_ROW_CLASS}>
                  <AdminTh>Organization and app</AdminTh>
                  {shown.map((column) => (
                    <AdminTh key={column.id} align='right'>
                      {column.label}
                    </AdminTh>
                  ))}
                </TableRow>
              </TableHeader>
              <TableBody>
                {listed.map((org) => {
                  const expanded = !compact || open.has(org.org_id);
                  return (
                    <Fragment key={org.org_id}>
                      <TableRow
                        id={`org-${org.org_id}`}
                        aria-current={org.org_id === target ? "location" : undefined}
                        className={cn(
                          "border-border/60 bg-muted/30 hover:bg-muted/30",
                          org.org_id === target &&
                            "outline outline-1 outline-primary -outline-offset-1"
                        )}
                        data-testid={`admin-usage-report-org-${org.org_id}`}
                      >
                        <TableCell className='font-medium text-xs'>
                          <span className='flex items-center gap-1.5'>
                            {compact && org.apps.length > 0 ? (
                              <button
                                type='button'
                                onClick={() => toggle(org.org_id)}
                                aria-expanded={expanded}
                                aria-label={`${expanded ? "Hide" : "Show"} ${org.name}'s apps`}
                                className='-ml-1 p-0.5 text-muted-foreground hover:text-foreground'
                                data-testid={`admin-usage-report-org-${org.org_id}-toggle`}
                              >
                                <ChevronRight
                                  className={cn(
                                    "size-3 transition-transform",
                                    expanded && "rotate-90"
                                  )}
                                />
                              </button>
                            ) : null}
                            {/* The org's own console: what its apps cost and how they
                                perform, over the same seven days. */}
                            <Link
                              to={`${ROUTES.ADMIN.OBSERVABILITY_ORG(org.org_id)}?days=7`}
                              className='hover:underline'
                              data-testid={`admin-usage-report-org-${org.org_id}-link`}
                            >
                              {org.name}
                            </Link>
                            {compact ? (
                              <span className='font-normal text-muted-foreground'>
                                {org.apps.length} app{org.apps.length === 1 ? "" : "s"}
                              </span>
                            ) : null}
                          </span>
                        </TableCell>
                        {shown.map((column) => (
                          <TableCell
                            key={column.id}
                            className='text-right font-medium text-xs tabular-nums'
                            data-testid={`admin-usage-report-org-${org.org_id}-${column.id}`}
                          >
                            {/* Blank where the report has no figure for an organization at
                                all, which is a different thing from one it could not measure. */}
                            {column.org ? figure(column.org(org), column) : null}
                          </TableCell>
                        ))}
                      </TableRow>
                      {expanded
                        ? org.apps.map((app) => (
                            <TableRow
                              key={app.app_id}
                              className='border-border/60 hover:bg-muted/40'
                              data-testid={`admin-usage-report-app-${app.app_id}`}
                            >
                              <TableCell className='pl-6 text-xs'>
                                <Link
                                  to={appConsolePath(org.slug, app.slug)}
                                  className='hover:underline'
                                >
                                  {app.name}
                                </Link>
                              </TableCell>
                              {shown.map((column) => (
                                <TableCell
                                  key={column.id}
                                  className='text-right text-xs tabular-nums'
                                  data-testid={`admin-usage-report-app-${app.app_id}-${column.id}`}
                                >
                                  {figure(column.app(app), column)}
                                </TableCell>
                              ))}
                            </TableRow>
                          ))
                        : null}
                    </Fragment>
                  );
                })}
              </TableBody>
            </Table>

            {quiet.length > 0 ? (
              <div
                className='flex items-center justify-between gap-3 border-border/60 border-t px-3 py-2 text-muted-foreground text-xs'
                data-testid='admin-usage-report-quiet'
              >
                <span>
                  {quiet.length} organization{quiet.length === 1 ? "" : "s"} had no activity this
                  week.
                </span>
                <button
                  type='button'
                  onClick={() => setShowQuiet((v) => !v)}
                  className='font-medium text-foreground underline-offset-2 hover:underline'
                  data-testid='admin-usage-report-quiet-toggle'
                >
                  {showQuiet ? "Hide" : "Show"}
                </button>
              </div>
            ) : null}

            {/* A column that is absent says why, once. */}
            {hidden.length > 0 ? (
              <p
                className='border-border/60 border-t px-3 py-2 text-muted-foreground text-xs'
                data-testid='admin-usage-report-hidden-columns'
              >
                {hidden.map((h) => `${h.label} ${h.verb} not shown because ${h.why}.`).join(" ")}
              </p>
            ) : null}
          </CardContent>
        </Card>
      )}
    </section>
  );
}
