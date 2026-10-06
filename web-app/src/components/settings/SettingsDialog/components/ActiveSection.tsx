import { CanOrgAdmin } from "@/components/auth/Can";
import type { SettingsSection } from "@/stores/useSettingsDialog";
import type { Organization, OrgRole } from "@/types/organization";
import type { Workspace } from "@/types/workspace";
import PersonalTokens from "../sections/account/PersonalTokens";
import OrgApiAccess from "../sections/organization/ApiAccess";
import OrgAppAccess from "../sections/organization/AppAccess";
import Billing from "../sections/organization/Billing";
import OrgCrew from "../sections/organization/Crew";
import General from "../sections/organization/General";
import Integration from "../sections/organization/Integration";
import OrgLocations from "../sections/organization/Locations";
import OrgMembers from "../sections/organization/Members";
import OrgPositions from "../sections/organization/Positions";
import OrgTeams from "../sections/organization/Teams";
import Appearance from "../sections/preferences/Appearance";
import ActivityLogs from "../sections/workspace/ActivityLogs";
import Airhouse from "../sections/workspace/Airhouse";
import ApiKeys from "../sections/workspace/ApiKeys";
import Apps from "../sections/workspace/Apps";
import Connections from "../sections/workspace/Connections";
import Databases from "../sections/workspace/Databases";
import LegacyApiKeys from "../sections/workspace/LegacyApiKeys";
import WorkspaceMembers from "../sections/workspace/Members";
import Oltp from "../sections/workspace/Oltp";
import OxyAccess from "../sections/workspace/OxyAccess";
import Previews from "../sections/workspace/Previews";
import Repositories from "../sections/workspace/Repositories";
import Secrets from "../sections/workspace/Secrets";
import NoAccessNotice from "./NoAccessNotice";

export type ActiveSectionProps = {
  activeSection: SettingsSection;
  org: Organization | null;
  role: OrgRole | null;
  workspace: Workspace | null;
  close: () => void;
};

/**
 * Renders whichever settings section matches `activeSection`. Shared between
 * the mobile and desktop layouts so section selection logic lives in one place.
 *
 * **The nav is the gate.** `activeSection` is always one of the items
 * `visibleNavGroups` produced, so a section the caller may not reach can never
 * become active. Nothing here re-derives authority — a second check against a
 * *different* role is what hid Secrets from workspace admins who were only org
 * members.
 *
 * A section's own `Can*` wrapper is defence-in-depth on top of that, not the
 * primary gate, and coverage is uneven: General, Databases, Apps, Secrets,
 * Repositories and ApiKeys carry one; Integration, Teams and App access do not
 * (they hide mutations via `viewerRole`/`canManage` instead). Billing and Oxy
 * access get their wrapper here because they have none internally. Anything
 * added below inherits the nav gate — give it a `requires` entry in `nav.ts`.
 */
export function ActiveSection({ activeSection, org, role, workspace, close }: ActiveSectionProps) {
  return (
    <>
      {org && role && activeSection === "organization.general" && (
        <General org={org} onClose={close} />
      )}
      {org && role && activeSection === "organization.members" && (
        <OrgMembers org={org} viewerRole={role} />
      )}
      {org && role && activeSection === "organization.teams" && (
        <OrgTeams org={org} viewerRole={role} />
      )}
      {org && role && activeSection === "organization.app_access" && (
        <OrgAppAccess org={org} viewerRole={role} />
      )}
      {org && role && activeSection === "organization.crew" && (
        <OrgCrew org={org} viewerRole={role} />
      )}
      {org && role && activeSection === "organization.locations" && (
        <OrgLocations org={org} viewerRole={role} />
      )}
      {org && role && activeSection === "organization.positions" && (
        <OrgPositions org={org} viewerRole={role} />
      )}
      {org && role && activeSection === "organization.api_access" && (
        <OrgApiAccess org={org} viewerRole={role} />
      )}
      {org && role && activeSection === "organization.billing" && (
        <CanOrgAdmin
          fallback={
            <NoAccessNotice>You need organization admin access to manage billing.</NoAccessNotice>
          }
        >
          <Billing org={org} onClose={close} />
        </CanOrgAdmin>
      )}
      {org && activeSection === "organization.integration" && <Integration org={org} />}

      {workspace && activeSection === "workspace.members" && <WorkspaceMembers />}
      {workspace && activeSection === "workspace.databases" && <Databases />}
      {workspace && activeSection === "workspace.airhouse" && <Airhouse />}
      {workspace && activeSection === "workspace.oltp" && <Oltp />}
      {workspace && activeSection === "workspace.repositories" && <Repositories />}
      {workspace && activeSection === "workspace.previews" && (
        <Previews workspace={workspace} onOpened={close} />
      )}
      {workspace && activeSection === "workspace.api_keys" && <ApiKeys />}
      {workspace && activeSection === "workspace.legacy_api_keys" && <LegacyApiKeys />}
      {workspace && activeSection === "workspace.secrets" && <Secrets />}
      {workspace && activeSection === "workspace.connections" && <Connections />}
      {workspace && activeSection === "workspace.apps" && <Apps />}
      {workspace && activeSection === "workspace.oxy_access" && (
        <CanOrgAdmin
          fallback={
            <NoAccessNotice>
              You need organization admin access to manage Oxy staff access.
            </NoAccessNotice>
          }
        >
          <OxyAccess />
        </CanOrgAdmin>
      )}
      {workspace && activeSection === "workspace.activity_logs" && <ActivityLogs />}

      {activeSection === "account.tokens" && <PersonalTokens />}

      {activeSection === "preferences.appearance" && <Appearance />}
    </>
  );
}
