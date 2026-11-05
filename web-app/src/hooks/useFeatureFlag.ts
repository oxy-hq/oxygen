import { useOrgFeatureFlags } from "@/hooks/api/featureFlags";
import useCurrentOrg from "@/stores/useCurrentOrg";

export interface FeatureFlagValue {
  /** The variant this org gets; `undefined` until the server has said. */
  variant: string | undefined;
  enabled: boolean;
  isLoading: boolean;
}

/**
 * Reads one `client` feature flag for the current org.
 *
 * A flag reads off (`enabled: false`, `variant: undefined`) until the server says
 * otherwise: no org, still loading, a failed fetch, or a key the server did not
 * return all look the same. The server does the evaluation and logs the A/B
 * exposure; the web app only reads the answer, so there is nothing to record here.
 */
export function useFeatureFlag(key: string): FeatureFlagValue {
  const orgId = useCurrentOrg((s) => s.org?.id);
  const { data, isLoading } = useOrgFeatureFlags(orgId);
  const flag = data?.flags[key];
  return {
    variant: flag?.variant,
    enabled: flag?.enabled ?? false,
    isLoading: !!orgId && isLoading
  };
}
