import { Users } from "lucide-react";
import { toast } from "sonner";
import { Card, CardContent } from "@/components/ui/shadcn/card";
import { Table, TableBody, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useSetUsageReportRecipient, useUsageReportRecipients } from "@/hooks/api/usageReport";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { apiErrorMessage } from "@/libs/apiError";
import { usageReportRecipientsReachable } from "@/pages/admin/AdminLayout/adminNav";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import type { UsageReportRecipient } from "@/types/usageReport";
import { UsageReportRecipientRow } from "./UsageReportRecipientRow";

/**
 * Everyone the Monday report is addressed to, with a switch per person.
 *
 * The section gates itself. Whether it renders and whether its list is fetched are read
 * off one value, so it cannot be mounted somewhere new with the fetch left ungated — and
 * the endpoint is narrower than the page (`manage_platform_grants`, not
 * `operate_platform`), so an ungated fetch is a 403 toast on every visit to Settings.
 *
 * No confirm step: it is one email a week, and the same switch turns it back on.
 */
export function UsageReportRecipients() {
  const { data: user } = useCurrentUser();
  const canManage = usageReportRecipientsReachable({
    isOwner: user?.is_owner ?? false,
    capabilities: user?.platform_capabilities ?? []
  });
  // The whole query, so a failed fetch is not mistaken for "nobody gets the report".
  const recipients = useUsageReportRecipients({ enabled: canManage });
  const setRecipient = useSetUsageReportRecipient();

  if (!canManage) return null;

  const toggle = (recipient: UsageReportRecipient, enabled: boolean) => {
    // The hook has already moved the switch and will move it back if this fails.
    setRecipient.mutate(
      { email: recipient.email, enabled },
      {
        onSuccess: (saved) => {
          toast.success(
            saved.enabled
              ? `Usage report emails turned on for ${saved.email}.`
              : `Usage report emails turned off for ${saved.email}.`
          );
        },
        onError: (err) => {
          toast.error(
            apiErrorMessage(err, `Couldn't change the usage report email for ${recipient.email}.`)
          );
        }
      }
    );
  };

  return (
    <section className='space-y-3' data-testid='admin-settings-recipients'>
      <div className='space-y-1'>
        <h3 className='font-semibold text-sm'>Who gets the usage report</h3>
        <p className='text-muted-foreground text-xs'>
          Global owners and global admins. Turn it off for someone who does not want it.
        </p>
      </div>
      <AdminAsync
        query={recipients}
        noun='the list of people who get the report'
        rows={3}
        isEmpty={(data) => data.recipients.length === 0}
        empty={
          <AdminEmptyState
            icon={Users}
            title='Nobody is set to get the report.'
            data-testid='admin-settings-recipients-empty'
          />
        }
      >
        {(data) => (
          <Card className='overflow-hidden'>
            <CardContent className='p-0'>
              <Table>
                <TableHeader>
                  <TableRow className={ADMIN_HEADER_ROW_CLASS}>
                    <AdminTh>Person</AdminTh>
                    <AdminTh>Role</AdminTh>
                    <AdminTh>Reach</AdminTh>
                    <AdminTh align='right'>Emailed</AdminTh>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {data.recipients.map((recipient) => (
                    <UsageReportRecipientRow
                      key={recipient.email}
                      recipient={recipient}
                      disabled={setRecipient.isPending}
                      onToggle={toggle}
                    />
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        )}
      </AdminAsync>
    </section>
  );
}
