import type { OxygenAccess } from "@/types/oxygenAccess";
import { apiClient } from "./axios";

const base = (orgId: string) => `/orgs/${orgId}/oxygen-access`;

/**
 * What the organization's custom apps ask of Oxygen and the org admin's
 * approval of it. Org admin only; approving and revoking also need a browser
 * session (a token answers 403 `session_required`).
 */
export class OxygenAccessService {
  static async list(orgId: string): Promise<OxygenAccess[]> {
    const response = await apiClient.get<{ apps: OxygenAccess[] }>(base(orgId));
    return response.data.apps ?? [];
  }

  static async get(orgId: string, appId: string): Promise<OxygenAccess> {
    const response = await apiClient.get<OxygenAccess>(`${base(orgId)}/${appId}`);
    return response.data;
  }

  /** Approve `actions` (every action the live build asks for when omitted). */
  static async approve(orgId: string, appId: string, actions?: string[]): Promise<OxygenAccess> {
    const response = await apiClient.post<OxygenAccess>(
      `${base(orgId)}/${appId}/approve`,
      actions ? { actions } : {}
    );
    return response.data;
  }

  static async revoke(orgId: string, appId: string): Promise<OxygenAccess> {
    const response = await apiClient.post<OxygenAccess>(`${base(orgId)}/${appId}/revoke`);
    return response.data;
  }
}
