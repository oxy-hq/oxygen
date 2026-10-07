import { Fragment, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { Card, CardContent } from "@/components/ui/shadcn/card";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import type { UsageOrg } from "@/types/usageReport";
import { type UsageColumn, usageColumns } from "../usageColumns";
import { appConsolePath } from "../utils";

/**
 * A row's figure in one column — or, where the row has none, the column's own words for
 * that, muted. "not measured" must not look like a number: it is the absence of one.
 */
const figure = (value: string | null, column: UsageColumn): ReactNode =>
  value ?? <span className='font-normal text-muted-foreground'>{column.missing}</span>;

/**
 * Every organization in the report and, under each, its apps — one table, so a number
 * can be read down its column across organizations.
 *
 * No colour on the change column. On this surface colour means something is wrong, and
 * the highlights above have already said what is; a fall of two people at a three-person
 * shop is not an alarm.
 */
export function UsageByOrganization({ orgs }: { orgs: readonly UsageOrg[] }) {
  const { shown, hidden } = usageColumns(orgs);

  return (
    <section className='space-y-2' data-testid='admin-usage-report-orgs'>
      <div className='space-y-1'>
        <h3 className='font-semibold text-sm'>By organization</h3>
        <p className='text-muted-foreground text-xs'>
          Figures are for the week of the report. Change is the number of people compared with the
          week before.
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
                {orgs.map((org) => (
                  <Fragment key={org.org_id}>
                    <TableRow
                      className='border-border/60 bg-muted/30 hover:bg-muted/30'
                      data-testid={`admin-usage-report-org-${org.org_id}`}
                    >
                      <TableCell className='font-medium text-xs'>{org.name}</TableCell>
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
                    {org.apps.map((app) => (
                      <TableRow
                        key={app.app_id}
                        className='border-border/60 hover:bg-muted/40'
                        data-testid={`admin-usage-report-app-${app.app_id}`}
                      >
                        <TableCell className='pl-6 text-xs'>
                          <Link to={appConsolePath(org.slug, app.slug)} className='hover:underline'>
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
                    ))}
                  </Fragment>
                ))}
              </TableBody>
            </Table>

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
