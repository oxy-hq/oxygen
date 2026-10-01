import { useMutation, useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { clearAssumeDestination, takeAssumeLanding } from "@/libs/utils/assumeDestination";
import { AdminAssumeService } from "@/services/api/adminAssume";
import queryKeys from "../queryKey";
import { landingFor, useActingSession } from "./useActingSession";

/**
 * Live assume-role sessions for the current user. Polled so the banner's
 * countdown stays honest and the banner disappears when the session expires
 * server-side (an expired session grants nothing, so the UI must not imply
 * otherwise).
 */
export const useCurrentAssume = (enabled = true) =>
  useQuery({
    queryKey: queryKeys.adminAssume.current(),
    queryFn: () => AdminAssumeService.current(),
    enabled,
    refetchInterval: 30_000,
    retry: false
  });

export const useStartAssume = () => {
  return useMutation({
    mutationFn: ({ orgId, reason }: { orgId: string; reason: string }) =>
      AdminAssumeService.start(orgId, reason),
    onSuccess: (s) => {
      // A HARD navigation (full page load), NOT a client-side one. Assuming a role
      // changes who every request is computed for; a soft navigate left stale +
      // in-flight queries (built for your old identity) firing and 403-ing — a
      // flash of "unauthorized" toasts and half-loaded pages. A full load re-inits
      // the whole app cleanly under the new session and lands where they live.
      //
      // An entry point that named a specific destination (assuming from one app's
      // admin page, say) wins over the org-shaped default: the operator asked to
      // see *that thing* with real data, and the admin page they're on 403s the
      // moment the session is live.
      window.location.assign(takeAssumeLanding(s.org_id) ?? landingFor(s));
    },
    onError: () => toast.error("Could not start the assume-role session")
  });
};

export const useEndAssume = () => {
  const { returnTo } = useActingSession();
  return useMutation({
    mutationFn: (orgId?: string) => AdminAssumeService.end(orgId),
    onSuccess: () => {
      // Hard navigation (not client-side), symmetric with starting a session:
      // ending it flips identity back to you, so stale + in-flight requests
      // computed AS THEM would 403 on the way out — the same "unauthorized" flash.
      // A full load reopens whichever console they came from — admin for staff,
      // the partner console for a partner (sending a partner to /admin would just
      // 403 them at the door) — cleanly as themselves. `returnTo` narrows that to
      // the exact page they left when the entry point recorded one.
      //
      // Cleared first: the round trip is over, and a stale record would redirect
      // an unrelated later session for the same org.
      clearAssumeDestination();
      window.location.assign(returnTo);
    },
    onError: () => toast.error("Could not end the assume-role session")
  });
};
