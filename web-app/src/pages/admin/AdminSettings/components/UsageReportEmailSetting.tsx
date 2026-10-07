import { useId } from "react";
import { Link } from "react-router-dom";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { Card, CardContent } from "@/components/ui/shadcn/card";
import { Switch } from "@/components/ui/shadcn/switch";
import { useSendUsageReportToMe, useSetUsageReportEmailPreference } from "@/hooks/api/usageReport";
import { apiErrorMessage } from "@/libs/apiError";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { UsageReportDelivery, UsageReportEmailPreference } from "@/types/usageReport";
import { sendFailureMessage } from "../sendFailureMessage";

/**
 * The Monday usage-report email, for the signed-in staff member only.
 *
 * No confirm step: it changes one person's own inbox and is undone with the same switch.
 */
export function UsageReportEmailSetting({
  preference
}: {
  preference: UsageReportEmailPreference;
}) {
  const setPreference = useSetUsageReportEmailPreference();
  const send = useSendUsageReportToMe();
  const switchId = useId();
  const helpId = useId();

  const toggle = (enabled: boolean) => {
    // The hook has already moved the switch and will move it back if this fails.
    setPreference.mutate(
      { enabled },
      {
        onSuccess: (saved) => {
          toast.success(
            saved.enabled ? "Usage report emails turned on." : "Usage report emails turned off."
          );
        },
        onError: (err) => {
          toast.error(apiErrorMessage(err, "Couldn't save your email preference."));
        }
      }
    );
  };

  const sendLatest = () => {
    send.mutate(undefined, {
      onSuccess: ({ outcome, to }) => {
        toast.success(outcome === "sent" ? `Sent to ${to}.` : "Opened a preview in the browser.");
      },
      onError: (err) => {
        toast.error(sendFailureMessage(err));
      }
    });
  };

  return (
    <div className='space-y-3'>
      <Card className='overflow-hidden'>
        <CardContent className='p-0'>
          <div className='flex items-start justify-between gap-4 p-4'>
            <div className='min-w-0 space-y-1'>
              <label htmlFor={switchId} className='block font-medium text-xs'>
                Custom app usage report
              </label>
              <p
                id={helpId}
                className='text-muted-foreground text-xs'
                data-testid='admin-settings-usage-report-help'
              >
                A weekly summary of how organizations used their custom apps, sent on Mondays to{" "}
                <span className='break-all text-foreground'>{preference.email}</span>.
              </p>
            </div>
            <Switch
              id={switchId}
              aria-describedby={helpId}
              checked={preference.enabled}
              onCheckedChange={toggle}
              disabled={setPreference.isPending}
              data-testid='admin-settings-usage-report-switch'
            />
          </div>
          <DeliveryNote delivery={preference.delivery} />
        </CardContent>
      </Card>

      <div className='flex flex-wrap items-center gap-x-4 gap-y-2'>
        <Button
          variant='outline'
          size='sm'
          className='h-7 px-2.5 text-xs'
          onClick={sendLatest}
          // Nothing can be sent without a sender, so the button says so by being off
          // rather than by failing with a 503 after the click.
          disabled={send.isPending || preference.delivery === "off"}
          data-testid='admin-settings-send-latest'
        >
          Send me the latest report
        </Button>
        <Link
          to={ROUTES.ADMIN.USAGE_REPORT}
          className='text-muted-foreground text-xs underline underline-offset-2 transition-colors hover:text-foreground'
          data-testid='admin-settings-open-report'
        >
          Open the latest report
        </Link>
      </div>
    </div>
  );
}

/**
 * Why the switch may not do what it says on this deployment.
 *
 * Without it, someone on a deployment with no sender turns the email on, waits for
 * Monday, and concludes the feature is broken.
 */
function DeliveryNote({ delivery }: { delivery: UsageReportDelivery }) {
  if (delivery === "email") return null;
  const off = delivery === "off";
  return (
    <p
      data-testid='admin-settings-delivery-note'
      data-delivery={delivery}
      className={cn(
        "border-border/60 border-t px-4 py-3 text-xs",
        off ? cn(ADMIN_TONE.warn.bg, ADMIN_TONE.warn.text) : "text-muted-foreground"
      )}
    >
      {off
        ? "This deployment has no email sender configured, so the report is not emailed. You can still read it under Usage report."
        : "This deployment previews email in the browser instead of sending it, so the Monday report is not delivered here."}
    </p>
  );
}
