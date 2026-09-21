import type {
  CreatedKioskDevice,
  CreateKioskDeviceRequest,
  EnrolledWorker,
  EnrolWorkerRequest,
  FrontlineLoginRequest,
  FrontlineLoginResponse,
  FrontlineRosterResponse,
  KioskDevice,
  KioskDeviceRow,
  ListDevicesResponse,
  ListWorkersResponse,
  UpdateKioskDeviceRequest,
  WorkerAppsResponse,
  WorkerStandingResponse
} from "@/types/frontline";
import { apiClient } from "./axios";

/**
 * Frontline (crew) workers: the kiosk side and the org-admin side of one feature.
 *
 * The three kiosk routes (`deviceStatus`, `roster`, `login`) are unauthenticated —
 * the kiosk cookie, not a bearer token, identifies the device — and sit in
 * `publicAPIPaths` so a wrong-PIN 401 renders inline instead of the interceptor
 * bouncing the page to `/login`. Everything under `/orgs/{orgId}/frontline/*` is
 * the admin side and needs an org-admin session.
 */
export class FrontlineService {
  /** Always 200: `{ bound: false }` when this browser holds no kiosk cookie. */
  static async deviceStatus(): Promise<KioskDevice> {
    const response = await apiClient.get("/frontline/device");
    return response.data;
  }

  /** Names to tap. Empty when the device isn't bound to `org` — never an error. */
  static async roster(org: string): Promise<FrontlineRosterResponse> {
    const response = await apiClient.get("/frontline/roster", { params: { org } });
    return response.data;
  }

  /**
   * 401 for every sign-in failure (wrong PIN, unknown identifier, locked out,
   * device not bound — one indistinguishable answer by design), 429 when the
   * org is rate-limited, 503 when the verifier is unavailable.
   */
  static async login(request: FrontlineLoginRequest): Promise<FrontlineLoginResponse> {
    const response = await apiClient.post("/frontline/login", request);
    return response.data;
  }

  // ── Org admin: workers ──

  /** Sorted by name. `apps` on each row are grant targets, not what's published. */
  static async listWorkers(orgId: string): Promise<ListWorkersResponse> {
    const response = await apiClient.get<ListWorkersResponse>(`/orgs/${orgId}/frontline/workers`);
    return response.data;
  }

  /**
   * 409 when the identifier is taken, 400 for a PIN outside 4–8 digits, a blank
   * field or an app that isn't this org's, 403 when the caller may not grant apps.
   */
  static async enrolWorker(orgId: string, request: EnrolWorkerRequest): Promise<EnrolledWorker> {
    const response = await apiClient.post<EnrolledWorker>(
      `/orgs/${orgId}/frontline/workers`,
      request
    );
    return response.data;
  }

  /** 404 when `userId` is not a worker of this org. */
  static async setWorkerStanding(
    orgId: string,
    userId: string,
    active: boolean
  ): Promise<WorkerStandingResponse> {
    const response = await apiClient.patch<WorkerStandingResponse>(
      `/orgs/${orgId}/frontline/workers/${userId}`,
      { active }
    );
    return response.data;
  }

  /** Full replace over this org's apps — an id left out is a grant removed. */
  static async setWorkerApps(
    orgId: string,
    userId: string,
    apps: string[]
  ): Promise<WorkerAppsResponse> {
    const response = await apiClient.put<WorkerAppsResponse>(
      `/orgs/${orgId}/frontline/workers/${userId}/apps`,
      { apps }
    );
    return response.data;
  }

  /** Replaces the PIN and clears any lockout. 204; 400 when the PIN isn't 4–8 digits. */
  static async resetWorkerPin(orgId: string, userId: string, pin: string): Promise<void> {
    await apiClient.post(`/orgs/${orgId}/frontline/workers/${userId}/pin`, { pin });
  }

  // ── Org admin: kiosks ──

  /** Newest first; revoked rows stay so the audit trail does. */
  static async listDevices(orgId: string): Promise<ListDevicesResponse> {
    const response = await apiClient.get<ListDevicesResponse>(`/orgs/${orgId}/frontline/devices`);
    return response.data;
  }

  /**
   * The response carries the enrol link exactly once. `return_to` must be an
   * absolute URL the deployment allows; omit it for the org home.
   */
  static async createDevice(
    orgId: string,
    request: CreateKioskDeviceRequest
  ): Promise<CreatedKioskDevice> {
    const response = await apiClient.post<CreatedKioskDevice>(
      `/orgs/${orgId}/frontline/devices`,
      request
    );
    return response.data;
  }

  /**
   * A new one-time link for a kiosk whose tablet never bound; the previous link
   * stops working. Same body as `createDevice`. 409 once bound or revoked.
   */
  static async reissueEnrolLink(
    orgId: string,
    deviceId: KioskDeviceRow["id"]
  ): Promise<CreatedKioskDevice> {
    const response = await apiClient.post<CreatedKioskDevice>(
      `/orgs/${orgId}/frontline/devices/${deviceId}/enrol-link`
    );
    return response.data;
  }

  /**
   * Changes an enrolled kiosk in place — the tablet keeps its cookie, so this
   * is the alternative to revoke-and-enrol for a wrong number.
   *
   * Send only what changes: `idle_timeout_seconds: null` clears the kiosk back
   * to the platform default, while leaving the field out keeps whatever is
   * there. 409 when the kiosk has been revoked.
   */
  static async updateDevice(
    orgId: string,
    deviceId: KioskDeviceRow["id"],
    request: UpdateKioskDeviceRequest
  ): Promise<KioskDeviceRow> {
    const response = await apiClient.patch<KioskDeviceRow>(
      `/orgs/${orgId}/frontline/devices/${deviceId}`,
      request
    );
    return response.data;
  }

  /** Revokes; the row remains with `revoked_at` set. */
  static async revokeDevice(orgId: string, deviceId: KioskDeviceRow["id"]): Promise<void> {
    await apiClient.delete(`/orgs/${orgId}/frontline/devices/${deviceId}`);
  }
}
