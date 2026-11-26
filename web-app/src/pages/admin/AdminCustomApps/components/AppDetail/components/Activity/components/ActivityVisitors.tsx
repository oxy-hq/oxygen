import { Users } from "lucide-react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAppActivityVisitors } from "@/hooks/api/customApps/useCustomApps";
import { AdminInfo } from "@/pages/admin/components/AdminInfo";
import type { VisitorRow } from "@/services/api/customApps";
import { relativeTime } from "../relativeTime";

/**
 * The visitor's standing **as recorded on their latest view**, not as resolved
 * now — the table snapshots roles precisely so a change today doesn't rewrite
 * what last month's activity looked like.
 *
 * The app role leads because it is the one that governs this app; the org role
 * is the parenthetical, and is shown only when it differs (an org admin who is
 * also an app admin is one fact stated twice).
 *
 * A dash is the only honest rendering of an absent role, and it covers two cases
 * the column cannot tell apart: never recorded (a view predating role capture,
 * or a failed lookup), and legitimately no grant — which on an org-wide app is
 * every viewer without an explicit or team grant, i.e. most rows. Neither is a
 * claim the data supports stating in words, so the slot stays neutral and the
 * org role beside it carries what the row does know.
 */
const VisitorRole = ({
  appRole,
  orgRole
}: {
  appRole: VisitorRow["app_role"];
  orgRole: VisitorRow["org_role"];
}) => {
  if (!appRole && !orgRole) {
    return (
      <span
        className='text-muted-foreground'
        title='No role recorded — this view predates role capture, or the lookup failed'
        data-testid='admin-app-activity-visitor-role'
      >
        —
      </span>
    );
  }
  return (
    <span className='flex items-baseline gap-1.5' data-testid='admin-app-activity-visitor-role'>
      {appRole ? (
        <span className='bg-muted px-1.5 py-0.5 font-medium'>{appRole}</span>
      ) : (
        // A dash, not "no app role". NULL means *not recorded*, and for an
        // org-wide app it is also what a viewer with no explicit or team grant
        // legitimately resolves to — the majority row, not an edge case. Either
        // way the column cannot assert an absence; the org role beside it
        // carries what the row does know.
        <span className='text-muted-foreground'>—</span>
      )}
      {orgRole && orgRole !== appRole && (
        <span className='text-muted-foreground'>org: {orgRole}</span>
      )}
    </span>
  );
};

/** Per-user roll-up over the last 7 days, sorted by recency. */
export const ActivityVisitors = ({ appId }: { appId: string }) => {
  const { data, isLoading } = useAppActivityVisitors(appId, 7);

  return (
    <section data-testid='admin-app-activity-visitors'>
      <h3 className='mb-2 flex items-center gap-1.5 font-medium text-muted-foreground text-xs uppercase tracking-wider'>
        <Users className='size-3.5' />
        Visitors (last 7 days)
      </h3>
      {isLoading ? (
        <Skeleton className='h-32 w-full' />
      ) : (data?.length ?? 0) === 0 ? (
        <p
          className='flex items-center gap-1.5 text-muted-foreground text-xs'
          data-testid='admin-app-activity-visitors-empty'
        >
          No views yet.
          <AdminInfo>
            A view is recorded when someone opens the app's HTML: page loads, not bundle assets or
            API fetches. It is automatic; an app needs no tracking code to appear here.
          </AdminInfo>
        </p>
      ) : (
        <div className='overflow-hidden border'>
          <table className='w-full text-xs'>
            <thead className='bg-muted/40 text-muted-foreground text-xs uppercase tracking-wider'>
              <tr>
                <th className='px-3 py-2 text-left font-medium'>Visitor</th>
                <th className='px-3 py-2 text-left font-medium'>Role</th>
                <th className='px-3 py-2 text-right font-medium'>Sessions</th>
                <th className='px-3 py-2 text-right font-medium'>Views</th>
                <th className='px-3 py-2 text-right font-medium'>First seen</th>
                <th className='px-3 py-2 text-right font-medium'>Last seen</th>
              </tr>
            </thead>
            <tbody>
              {data?.map((v) => (
                <tr key={v.user_id} className='border-t' data-testid='admin-app-activity-visitor'>
                  <td className='px-3 py-2 font-mono text-xs'>{v.user_email}</td>
                  <td className='px-3 py-2'>
                    <VisitorRole appRole={v.app_role} orgRole={v.org_role} />
                  </td>
                  <td className='px-3 py-2 text-right'>{v.sessions}</td>
                  <td className='px-3 py-2 text-right'>{v.views}</td>
                  <td className='px-3 py-2 text-right text-muted-foreground text-xs'>
                    {relativeTime(v.first_seen_at)}
                  </td>
                  <td className='px-3 py-2 text-right text-muted-foreground text-xs'>
                    {relativeTime(v.last_seen_at)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
};
