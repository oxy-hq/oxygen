import { ArrowLeft, CalendarClock, Mail } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { useUsageReport } from "@/hooks/api/usageReport";
import ROUTES from "@/libs/utils/routes";
import { AdminAsync } from "../../components/AdminAsync";
import { AdminEmptyState } from "../../components/AdminEmptyState";
import { AdminPage } from "../../components/AdminPage";
import { UsageByOrganization } from "./components/UsageByOrganization";
import { UsageHighlights } from "./components/UsageHighlights";
import { UsageSummary } from "./components/UsageSummary";

/**
 * `/admin/observability/weekly` — how each organization used its custom apps last week.
 *
 * The same report the server emails to global admins every Monday. Its sentences and its
 * ordering are the server's, shown as written, so the page and the email cannot disagree
 * about what happened.
 *
 * It was a page of its own, `/admin/usage-report`, beside Observability in the rail. The
 * two answered adjacent questions about the same apps in two places — "what did they use
 * last week" there, "what do they cost and how do they perform" here — so the report is
 * now a view of Observability, routed rather than a tab (`pages/admin/CLAUDE.md`), and the
 * old route redirects here with the email's `#org-` anchor intact.
 */
export function WeeklyUsage() {
  // The whole query, never `data?.report ?? null`: that default would make a failed fetch
  // read "No report yet." — a server that is down and a console too new to have a report
  // would look identical, and only one of them is fine to walk away from.
  const report = useUsageReport();

  return (
    <AdminPage
      width='wide'
      description='How organizations used their custom apps last week, and what changed. A new report is written every Monday and emailed to staff.'
      actions={
        <div className='flex items-center gap-2'>
          <Button asChild variant='ghost' size='sm' className='h-7 gap-1.5 text-xs'>
            <Link to={ROUTES.ADMIN.OBSERVABILITY} data-testid='admin-usage-report-cost'>
              <ArrowLeft className='size-3.5' />
              Cost to serve
            </Link>
          </Button>
          <Button asChild variant='outline' size='sm' className='h-7 gap-1.5 px-2.5 text-xs'>
            <Link to={ROUTES.ADMIN.SETTINGS} data-testid='admin-usage-report-email-settings'>
              <Mail className='size-3' />
              Email settings
            </Link>
          </Button>
        </div>
      }
      data-testid='admin-usage-report'
    >
      <AdminAsync
        query={report}
        noun='the usage report'
        rows={6}
        isEmpty={(data) => data.report === null}
        empty={
          <AdminEmptyState
            icon={CalendarClock}
            title='No report yet.'
            description='The first one is written on a Monday, once a full week has ended.'
            data-testid='admin-usage-report-empty'
          />
        }
      >
        {({ report: latest }) =>
          latest ? (
            <div className='space-y-6' data-testid='admin-usage-report-body'>
              <UsageSummary report={latest} />
              <UsageHighlights highlights={latest.highlights} />
              <UsageByOrganization orgs={latest.orgs} />
            </div>
          ) : null
        }
      </AdminAsync>
    </AdminPage>
  );
}
