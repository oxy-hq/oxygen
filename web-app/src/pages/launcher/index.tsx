import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import CustomerHome from "./CustomerHome";
import HqLauncher from "./HqLauncher";

/**
 * The home (`/<org>/home`, and a workspace's index and `home`). With the apps-first
 * experience on it is `CustomerHome`; off, the HQ launcher exactly as it was.
 * The HQ launcher goes once the flag is on everywhere.
 */
export default function LauncherPage() {
  return useAppsFirstExperience() ? <CustomerHome /> : <HqLauncher />;
}
