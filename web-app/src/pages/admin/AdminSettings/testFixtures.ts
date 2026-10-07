// Builders for the settings tests. Not imported by anything that ships.
import type { UsageReportEmailPreference, UsageReportRecipient } from "@/types/usageReport";

/** The caller's own preference: on, on a deployment that really sends email. */
export const preference = (
  over: Partial<UsageReportEmailPreference> = {}
): UsageReportEmailPreference => ({
  email: "luong@oxy.tech",
  enabled: true,
  delivery: "email",
  ...over
});

/** A global admin who reaches every organization, gets the email, and never changed it. */
export const recipient = (over: Partial<UsageReportRecipient> = {}): UsageReportRecipient => ({
  email: "ada@oxy.tech",
  role: "global_admin",
  scope_all: true,
  org_count: null,
  enabled: true,
  is_self: false,
  updated_by: null,
  updated_at: null,
  ...over
});
