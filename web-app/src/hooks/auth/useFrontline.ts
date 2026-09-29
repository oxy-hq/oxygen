import { queryOptions, useMutation, useQuery } from "@tanstack/react-query";
import queryKeys from "@/hooks/api/queryKey";
import { rememberKioskBrowser } from "@/libs/utils/kioskBrowser";
import { FrontlineService } from "@/services/api";
import type {
  FrontlineLoginRequest,
  FrontlineLoginResponse,
  FrontlineStaff,
  KioskDevice
} from "@/types/frontline";
import { resolveReturnTo } from "./postLoginRedirect";

const UNBOUND: KioskDevice = { bound: false };

/**
 * The kiosk probe, shared by every reader so they share one cache entry.
 * Never throws: a failed probe — a 503 when the server could not look the
 * kiosk up included — reads as "not a kiosk", so the ordinary login page
 * renders untouched. A "bound" answer is remembered for the kiosk session
 * check; nothing the probe answers ever forgets it (`forgetKioskBrowser` is for
 * leaving and revoking, which know the kiosk is gone).
 */
export const kioskDeviceQueryOptions = queryOptions<KioskDevice>({
  queryKey: queryKeys.frontline.device(),
  queryFn: async () => {
    let device: KioskDevice;
    try {
      device = await FrontlineService.deviceStatus();
    } catch (error: unknown) {
      console.warn("Kiosk device probe failed; treating this browser as unbound", error);
      return UNBOUND;
    }
    if (device.bound) {
      rememberKioskBrowser();
    }
    return device;
  },
  staleTime: 60_000,
  retry: false
});

/** Whether this browser is an enrolled kiosk. */
export const useKioskDevice = () => useQuery<KioskDevice>(kioskDeviceQueryOptions);

/** The names on this kiosk's shift board. Only asks once the org is known. */
export const useFrontlineRoster = (org: string | undefined) =>
  useQuery<FrontlineStaff[]>({
    queryKey: queryKeys.frontline.roster(org ?? ""),
    queryFn: async () => {
      if (!org) {
        return [];
      }
      const { staff } = await FrontlineService.roster(org);
      return staff;
    },
    enabled: Boolean(org),
    staleTime: 60_000
  });

/**
 * PIN sign-in. Post-login routing is the caller's job (see
 * {@link resolveCrewDestination}); the response carries no `UserInfo`, so it
 * must not be handed to `AuthContext.login`.
 */
export const useFrontlineLogin = () =>
  useMutation<FrontlineLoginResponse, Error, FrontlineLoginRequest>({
    mutationFn: FrontlineService.login
  });

/**
 * The only three things the crew page may say about a failed sign-in. The
 * server already collapses wrong PIN / unknown name / locked out / unbound
 * device into one 401, and this keeps the page from re-deriving a reason.
 */
export type CrewSignInFailure = "mismatch" | "rate_limited" | "unavailable";

export const CREW_SIGN_IN_MESSAGES: Record<CrewSignInFailure, string> = {
  mismatch: "That didn't match. Try again.",
  rate_limited: "Too many attempts on this kiosk. Wait a minute.",
  unavailable: "Sign-in isn't available right now. Try again in a moment."
};

export function classifyCrewSignInError(error: unknown): CrewSignInFailure {
  const status = (error as { response?: { status?: number } })?.response?.status;
  if (status === 401) {
    return "mismatch";
  }
  if (status === 429) {
    return "rate_limited";
  }
  return "unavailable";
}

/**
 * Where a worker lands once the PIN is accepted: the `return_to` the app sent
 * them here with, else the app this kiosk was enrolled for. Both go through the
 * server's return-to allowlist; `null` means this kiosk has nothing to open.
 */
export async function resolveCrewDestination(
  returnTo: string | null | undefined,
  deviceReturnTo: string | null | undefined
): Promise<string | null> {
  return (await resolveReturnTo(returnTo)) ?? resolveReturnTo(deviceReturnTo);
}

/**
 * True when a login `return_to` points at a custom app — the only place a crew
 * member could have been sent to this page from. Either subdomain scheme
 * counts: the app-host path (`/customer-apps/…`) or the custom-app subdomain
 * (`<org>--<slug>.customer-apps.…`).
 */
export function returnToPointsAtCustomApp(returnTo: string | null | undefined): boolean {
  if (!returnTo) {
    return false;
  }
  try {
    const url = new URL(returnTo, window.location.origin);
    return url.pathname.includes("/customer-apps/") || url.hostname.includes("customer-apps.");
  } catch {
    return false;
  }
}
