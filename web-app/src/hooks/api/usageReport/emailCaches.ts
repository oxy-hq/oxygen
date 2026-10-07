import type { QueryClient } from "@tanstack/react-query";
import type {
  UsageReportEmailPreference,
  UsageReportRecipient,
  UsageReportRecipientsResponse
} from "@/types/usageReport";
import queryKeys from "../queryKey";

/**
 * "Is this person emailed the report" is held in two caches: the caller's own preference,
 * and the list of everyone who gets it. The caller's row in that list is the same setting
 * as their preference, so a change made through either has to show in both — or the
 * switch at the top of Settings and the caller's own row disagree on one screen.
 *
 * Both mutations go through these helpers so neither can update one cache and forget the
 * other.
 */
export interface EmailCaches {
  own: UsageReportEmailPreference | undefined;
  list: UsageReportRecipientsResponse | undefined;
}

const ownKey = () => queryKeys.usageReport.emailPreference();
const listKey = () => queryKeys.usageReport.recipients();

/** Stop any read of either that is in flight, then hand back what they hold. */
export async function readEmailCaches(qc: QueryClient): Promise<EmailCaches> {
  await Promise.all([
    qc.cancelQueries({ queryKey: ownKey() }),
    qc.cancelQueries({ queryKey: listKey() })
  ]);
  return {
    own: qc.getQueryData<UsageReportEmailPreference>(ownKey()),
    list: qc.getQueryData<UsageReportRecipientsResponse>(listKey())
  };
}

/** Show `enabled` on the caller's own preference. Nothing is invented if it is not loaded. */
export function showOwnPreference(qc: QueryClient, own: EmailCaches["own"], enabled: boolean) {
  if (own) qc.setQueryData<UsageReportEmailPreference>(ownKey(), { ...own, enabled });
}

/**
 * Show `enabled` on the rows `match` picks.
 *
 * Who changed it, and when, are cleared rather than kept: until the server answers they
 * describe the change before this one, and a row turned off a second ago would read
 * "Turned off by" whoever touched it last month.
 */
export function showRecipients(
  qc: QueryClient,
  list: EmailCaches["list"],
  match: (recipient: UsageReportRecipient) => boolean,
  enabled: boolean
) {
  if (!list) return;
  qc.setQueryData<UsageReportRecipientsResponse>(listKey(), {
    ...list,
    recipients: list.recipients.map((r) =>
      match(r) ? { ...r, enabled, updated_by: null, updated_at: null } : r
    )
  });
}

/** Put both back as they were before a change the server refused. */
export function restoreEmailCaches(qc: QueryClient, previous: EmailCaches | undefined) {
  if (previous?.own) qc.setQueryData(ownKey(), previous.own);
  if (previous?.list) qc.setQueryData(listKey(), previous.list);
}

/**
 * Re-read both from the server. Only a query someone is showing is fetched, so this does
 * not ask for the list on behalf of a person who is not allowed to see it.
 */
export function refreshEmailCaches(qc: QueryClient) {
  qc.invalidateQueries({ queryKey: ownKey() });
  qc.invalidateQueries({ queryKey: listKey() });
}
