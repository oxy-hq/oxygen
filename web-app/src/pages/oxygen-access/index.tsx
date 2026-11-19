import { useParams } from "react-router-dom";
import { OxygenAccessApproval } from "@/components/settings/SettingsDialog/sections/organization/ApiAccess/OxygenAccess";
import useCurrentOrg from "@/stores/useCurrentOrg";

/**
 * `/<org>/oxygen-access/<appId>`: one app's request to act in Oxygen itself,
 * where an org owner or admin approves or revokes it. A page of its own rather
 * than a Settings tab, so a notification or an app's own refusal can link to
 * one request. The list of every app is the Apps tab of Settings → API tokens.
 */
export default function OxygenAccessPage() {
  const { appId = "" } = useParams<{ appId: string }>();
  const org = useCurrentOrg((s) => s.org);
  const role = useCurrentOrg((s) => s.role);
  const canManage = role === "owner" || role === "admin";

  return (
    <div
      className='mx-auto flex w-full max-w-xl flex-col gap-4 p-6'
      data-testid='oxygen-access-page'
    >
      <h1 className='text-lg font-semibold'>Apps</h1>
      {!org ? null : canManage ? (
        <OxygenAccessApproval orgId={org.id} appId={appId} />
      ) : (
        <p className='text-sm text-muted-foreground'>
          An owner or admin of this organization approves what an app may do in Oxygen.
        </p>
      )}
    </div>
  );
}
