import { KeySquare } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import useSettingsDialog from "@/stores/useSettingsDialog";
import type { Organization, OrgRole } from "@/types/organization";
import SectionHeader from "../../../components/SectionHeader";
import type { ApiTokensTab } from "../../../tabs";
import PersonalTokens from "../../account/PersonalTokens";
import { ApiIdField } from "./components/ApiIdField";
import { TokenGuide } from "./components/TokenGuide";
import { Inventory } from "./Inventory";
import { OxygenAccessList } from "./OxygenAccess";
import { Policy } from "./Policy";
import { ServiceAccounts } from "./ServiceAccounts";

interface ApiTokensSectionProps {
  /** `null` when no org is loaded: the page then holds only the viewer's own tokens. */
  org: Organization | null;
  viewerRole: OrgRole | null;
}

interface TabContext {
  /** Set for every tab but Personal access tokens, which are shown only with an org loaded. */
  org: Organization | null;
  /** The service account open in the Service accounts tab, if any. */
  accountId: string | null;
  setAccountId: (id: string | null) => void;
  /** Open one account's page from anywhere on this page. */
  openAccount: (id: string) => void;
}

interface TabDef {
  id: ApiTokensTab;
  label: string;
  /** Organization owners and admins only: every route behind the tab checks that role. */
  orgAdminOnly: boolean;
  testId: string;
  render: (ctx: TabContext) => React.ReactNode;
}

/**
 * The page's tabs, in order. A new tab is one entry here plus its id in `API_TOKENS_TABS`
 * (`tabs.ts`), which is what makes it linkable as `?settings=organization.api_tokens.<id>`.
 *
 * The test ids of the three admin tabs keep their `api-access-` names from when this page was
 * Organization → API access, because the authored browser flows find them by those.
 */
const TABS: readonly TabDef[] = [
  {
    id: "personal",
    label: "Personal access tokens",
    orgAdminOnly: false,
    testId: "api-tokens-tab-personal",
    render: () => <PersonalTokens />
  },
  {
    id: "service_accounts",
    label: "Service accounts",
    orgAdminOnly: true,
    testId: "api-access-tab-service-accounts",
    render: ({ org, accountId, setAccountId }) =>
      org && <ServiceAccounts org={org} selectedId={accountId} onSelect={setAccountId} />
  },
  {
    id: "apps",
    label: "Apps",
    orgAdminOnly: true,
    testId: "api-tokens-tab-apps",
    render: ({ org }) => org && <OxygenAccessList org={org} />
  },
  {
    id: "all",
    label: "All tokens",
    orgAdminOnly: true,
    testId: "api-access-tab-tokens",
    render: ({ org, openAccount }) => org && <Inventory org={org} onOpenAccount={openAccount} />
  },
  {
    id: "policy",
    label: "Policy",
    orgAdminOnly: true,
    testId: "api-access-tab-policy",
    render: ({ org }) => org && <Policy org={org} />
  }
];

/**
 * Organization → API tokens: every token that lets something call Oxygen without anyone signing
 * in, in one place. It replaced three pages — Account → Personal access tokens, Organization →
 * API access and Workspace → API tokens — whose old `?settings=` ids redirect to the matching tab
 * (`RENAMED_SECTIONS` in `nav.ts`).
 *
 * - **Personal access tokens**: the viewer's own, across every org. Every member.
 * - **Service accounts**: identities the org owns, their tokens, and the GitHub Actions
 *   workflows trusted to act as them. Org admins.
 * - **Apps**: custom apps that ask to act in Oxygen itself (`ctx.oxygen`), and the approval
 *   that lets them, with no token stored. Org admins.
 * - **All tokens**: every credential reaching the org, whoever holds it. Org admins.
 * - **Policy**: the org's rules for those tokens. Org admins.
 *
 * The nav item is ungated, because a personal token is always its owner's to manage, and it needs
 * no org: with none loaded it sits under Account and shows the personal tab alone. The admin tabs
 * are gated here by the same org role their routes check, so a member sees the guide and their
 * own tokens and nothing an admin route would refuse to fill.
 */
export default function ApiTokensSection({ org, viewerRole }: ApiTokensSectionProps) {
  const requested = useSettingsDialog((s) => s.tab);
  const setTab = useSettingsDialog((s) => s.setTab);
  // Lives here rather than in the Service accounts tab so it survives a tab switch, and so the
  // inventory can open an account's page from a token row.
  const [accountId, setAccountId] = useState<string | null>(null);
  const isOrgAdmin = !!org && (viewerRole === "owner" || viewerRole === "admin");

  const visible = TABS.filter((tab) => !tab.orgAdminOnly || isOrgAdmin);
  const active = visible.find((tab) => tab.id === requested) ?? visible[0];

  const ctx: TabContext = {
    org,
    accountId,
    setAccountId,
    openAccount: (id) => {
      setAccountId(id);
      setTab("service_accounts");
    }
  };

  return (
    <div className='flex flex-col gap-5' data-testid='settings-api-tokens'>
      <SectionHeader
        icon={KeySquare}
        title='API tokens'
        description='A token lets a script, a scheduled job or another service call Oxygen without anyone signing in.'
      />

      <TokenGuide visibleTabs={visible.map((tab) => tab.id)} />

      {visible.length === 1 ? (
        <div className='flex flex-col gap-3'>
          <h4 className='font-medium text-sm'>{active.label}</h4>
          {active.render(ctx)}
        </div>
      ) : (
        <Tabs value={active.id} onValueChange={(value) => setTab(value)} className='gap-4'>
          <TabsList className='w-fit max-w-full overflow-x-auto'>
            {visible.map((tab) => (
              <TabsTrigger
                key={tab.id}
                value={tab.id}
                className='px-3 text-xs'
                data-testid={tab.testId}
              >
                {tab.label}
              </TabsTrigger>
            ))}
          </TabsList>
          {visible.map((tab) => (
            <TabsContent key={tab.id} value={tab.id}>
              {tab.render(ctx)}
            </TabsContent>
          ))}
        </Tabs>
      )}

      <ApiIdField />
    </div>
  );
}
