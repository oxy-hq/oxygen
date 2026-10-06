import { FolderKey } from "lucide-react";
import type React from "react";
import { useAuth } from "@/contexts/AuthContext";
import { useRole } from "@/hooks/useRole";
import useSettingsDialog from "@/stores/useSettingsDialog";
import SectionHeader from "../../../components/SectionHeader";
import LegacyKeyNotice from "./components/LegacyKeyNotice";
import LegacyKeyTable from "./components/LegacyKeyTable";

/**
 * Workspace → Legacy API keys: the old `oxy_<hex>` keys, on the `/{workspaceId}/api-keys` routes.
 * The only place they are listed for their workspace, and the only place they are extended,
 * inspected or revoked. Nothing is created here: new credentials are API tokens, made under
 * Account. The create endpoint itself stays, for scripts.
 *
 * OPEN TO EVERY MEMBER OF THE WORKSPACE, not only its admins. A legacy API key belongs to its
 * owner: the server lists it, extends it and shows its activity to whoever owns it, with no
 * admin requirement, and the expiry email it sends links here. So the section carries no role
 * wrapper and no nav gate. The one thing an owner may not do without the workspace admin role is
 * revoke, and that is gated on the button itself (see RevokeAction), not on the section.
 */
const LegacyApiKeys: React.FC = () => {
  const { isLocalMode } = useAuth();
  const { is } = useRole();
  const openSettings = useSettingsDialog((s) => s.open);

  return (
    <div className='flex flex-col gap-5' data-testid='settings-legacy-api-keys'>
      <SectionHeader
        icon={FolderKey}
        title='Legacy API keys'
        description={
          is.workspaceAdmin
            ? "The older kind of API key. Extend one, see what it has done, or revoke it."
            : "The older kind of API key. Extend one, or see what it has done."
        }
      />

      <LegacyKeyNotice
        // Local mode has no accounts, so there is no Account section to open.
        onCreateToken={isLocalMode ? undefined : () => openSettings("account.tokens")}
      />

      <LegacyKeyTable />
    </div>
  );
};

export default LegacyApiKeys;
