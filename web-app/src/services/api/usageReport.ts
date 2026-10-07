import type {
  UsageReportEmailPreference,
  UsageReportRecipient,
  UsageReportRecipientsResponse,
  UsageReportResponse,
  UsageReportSendResult
} from "@/types/usageReport";
import { apiClient } from "./axios";

/**
 * The weekly custom-app usage report.
 *
 * Two gates, not one. Reading the report and setting your own email need
 * `operate_platform`; the two `recipients` routes decide what another person is sent, so
 * they need `manage_platform_grants`.
 */
export class UsageReportService {
  static async latest(): Promise<UsageReportResponse> {
    const response = await apiClient.get<UsageReportResponse>("/admin/usage-report");
    return response.data;
  }

  static async getEmailPreference(): Promise<UsageReportEmailPreference> {
    const response = await apiClient.get<UsageReportEmailPreference>(
      "/admin/usage-report/email-preference"
    );
    return response.data;
  }

  static async setEmailPreference(enabled: boolean): Promise<UsageReportEmailPreference> {
    const response = await apiClient.put<UsageReportEmailPreference>(
      "/admin/usage-report/email-preference",
      { enabled }
    );
    return response.data;
  }

  /** Answers 404 when no report exists yet, and 503 when the deployment has no sender. */
  static async sendToMe(): Promise<UsageReportSendResult> {
    const response = await apiClient.post<UsageReportSendResult>("/admin/usage-report/send-to-me");
    return response.data;
  }

  /** Everyone the report is addressed to, in the server's order. */
  static async recipients(): Promise<UsageReportRecipientsResponse> {
    const response = await apiClient.get<UsageReportRecipientsResponse>(
      "/admin/usage-report/recipients"
    );
    return response.data;
  }

  /**
   * Turn the email on or off for one person. Answers 404 (`not_a_recipient`) when the
   * address does not get the report.
   *
   * The address is a path segment, so it is encoded: an `@` or a plus-address `+` left
   * raw would reach the server as a different path.
   */
  static async setRecipient(email: string, enabled: boolean): Promise<UsageReportRecipient> {
    const response = await apiClient.put<UsageReportRecipient>(
      `/admin/usage-report/recipients/${encodeURIComponent(email)}`,
      { enabled }
    );
    return response.data;
  }
}
