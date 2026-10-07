import { CalendarClock, Mail } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { useUsageReport } from "@/hooks/api/usageReport";
import ROUTES from "@/libs/utils/routes";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { UsageByOrganization } from "./components/UsageByOrganization";
import { UsageHighlights } from "./components/UsageHighlights";
import { UsageSummary } from "./components/UsageSummary";

/**
 * `/admin/usage-report` — how each organization used its custom apps last week.
 *
 * The same report the server emails to global admins every Monday. Its sentences and its
 * ordering are the server's, shown as written, so the page and the email cannot disagree
 * about what happened.
 */
export default function AdminUsageReport() {
  // The whole query, never `data?.report ?? null`: that default would make a failed fetch
  // read "No report yet." — a server that is down and a console too new to have a report
  // would look identical, and only one of them is fine to walk away from.
  const report = useUsageReport();

  return (
    <AdminPage
      width='wide'
      description='How organizations used their custom apps last week, and what changed. A new report is written every Monday.'
      actions={
        <Button asChild variant='outline' size='sm' className='h-7 gap-1.5 px-2.5 text-xs'>
          <Link to={ROUTES.ADMIN.SETTINGS} data-testid='admin-usage-report-email-settings'>
            <Mail className='size-3' />
            Email settings
          </Link>
        </Button>
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
