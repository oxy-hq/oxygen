import { Switch } from "@/components/ui/shadcn/switch";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import type { UsageReportRecipient } from "@/types/usageReport";
import { reachLabel, roleLabel, turnedOffBy } from "../recipientText";

/** One person the report is addressed to, and the switch that decides whether it is sent. */
export function UsageReportRecipientRow({
  recipient,
  disabled,
  onToggle
}: {
  recipient: UsageReportRecipient;
  /** A save is in flight. */
  disabled: boolean;
  onToggle: (recipient: UsageReportRecipient, enabled: boolean) => void;
}) {
  const { email } = recipient;
  const note = turnedOffBy(recipient);

  return (
    <TableRow
      className='border-border/60 hover:bg-muted/40'
      data-testid={`admin-settings-recipient-${email}`}
    >
      {/* Wraps: an address is as long as it is, and the line under it is a sentence. */}
      <TableCell className='whitespace-normal text-xs'>
        <p className='flex flex-wrap items-baseline gap-x-2'>
          <span className='break-all'>{email}</span>
          {recipient.is_self ? (
            <span
              className='text-muted-foreground'
              data-testid={`admin-settings-recipient-${email}-self`}
            >
              you
            </span>
          ) : null}
        </p>
        {note ? (
          <p
            className='mt-0.5 break-words text-muted-foreground'
            data-testid={`admin-settings-recipient-${email}-turned-off-by`}
          >
            {note}
          </p>
        ) : null}
      </TableCell>
      <TableCell
        className='text-muted-foreground text-xs'
        data-testid={`admin-settings-recipient-${email}-role`}
      >
        {roleLabel(recipient.role)}
      </TableCell>
      <TableCell
        className='text-muted-foreground text-xs'
        data-testid={`admin-settings-recipient-${email}-reach`}
      >
        {reachLabel(recipient)}
      </TableCell>
      <TableCell className='text-right'>
        <Switch
          checked={recipient.enabled}
          onCheckedChange={(enabled) => onToggle(recipient, enabled)}
          disabled={disabled}
          aria-label={`Email the usage report to ${email}`}
          data-testid={`admin-settings-recipient-${email}-switch`}
        />
      </TableCell>
    </TableRow>
  );
}
