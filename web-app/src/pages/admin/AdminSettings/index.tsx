import { useUsageReportEmailPreference } from "@/hooks/api/usageReport";
import { AdminAsync } from "../components/AdminAsync";
import { AdminPage } from "../components/AdminPage";
import { UsageReportEmailSetting } from "./components/UsageReportEmailSetting";
import { UsageReportRecipients } from "./components/UsageReportRecipients";

/**
 * `/admin/settings` — the signed-in staff member's own preferences for this console.
 *
 * Not a rail entry: the rail lists surfaces of the platform, and this is about one
 * person. It is reached from the rail footer's user menu and from the usage report.
 */
export default function AdminSettings() {
  // The whole query: a default of "enabled" would draw a switch nobody has read from the
  // server, and flipping it would save the opposite of a setting we never saw.
  const preference = useUsageReportEmailPreference();

  return (
    <AdminPage
      width='narrow'
      description='Email preferences for this console.'
      data-testid='admin-settings'
    >
      <section className='space-y-3' data-testid='admin-settings-email'>
        <h3 className='font-semibold text-sm'>Email notifications</h3>
        <AdminAsync query={preference} noun='your email preference' rows={2}>
          {(loaded) => <UsageReportEmailSetting preference={loaded} />}
        </AdminAsync>
      </section>

      {/* The same email, for everyone else it goes to. Renders nothing — and fetches
          nothing — for someone who may not decide that; the section holds its own gate. */}
      <UsageReportRecipients />
    </AdminPage>
  );
}
