import { KeySquare } from "lucide-react";
import { useState } from "react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import type { Organization, OrgRole } from "@/types/organization";
import NoAccessNotice from "../../../components/NoAccessNotice";
import SectionHeader from "../../../components/SectionHeader";
import { Inventory } from "./Inventory";
import { Policy } from "./Policy";
import { ServiceAccounts } from "./ServiceAccounts";

interface ApiAccessSectionProps {
  org: Organization;
  viewerRole: OrgRole;
}

type ApiAccessTab = "service-accounts" | "tokens" | "policy";

/**
 * Organization → API access: everything that reaches the org without a person
 * at a keyboard.
 *
 * - **Service accounts**: identities the org owns, their tokens, and the
 *   GitHub Actions workflows trusted to act as them.
 * - **Tokens**: the inventory of every credential reaching the org, whoever
 *   holds it.
 * - **Policy**: the org's rules for those tokens.
 *
 * The nav gate (`requires: "orgAdmin"`) is the primary check. `canManage` is
 * the second line, and because the tabs render nothing without it, it also
 * holds every admin-only read back from a member who arrives by deep link.
 */
export default function ApiAccessSection({ org, viewerRole }: ApiAccessSectionProps) {
  const [tab, setTab] = useState<ApiAccessTab>("service-accounts");
  // Lives here rather than in the Service accounts tab so it survives a tab
  // switch, and so the inventory can open an account's page from a token row.
  const [accountId, setAccountId] = useState<string | null>(null);
  const canManage = viewerRole === "owner" || viewerRole === "admin";

  if (!canManage) {
    return (
      <NoAccessNotice>
        You need to be an organization owner or admin to manage API access.
      </NoAccessNotice>
    );
  }

  const openAccount = (id: string) => {
    setAccountId(id);
    setTab("service-accounts");
  };

  return (
    <div className='flex flex-col gap-5' data-testid='settings-api-access'>
      <SectionHeader
        icon={KeySquare}
        title='API access'
        description='What can reach this organization without someone signing in: service accounts, the tokens they and your members hold, and the rules those tokens follow.'
      />

      <Tabs value={tab} onValueChange={(value) => setTab(value as ApiAccessTab)} className='gap-4'>
        <TabsList className='w-fit'>
          <TabsTrigger
            value='service-accounts'
            className='px-3 text-xs'
            data-testid='api-access-tab-service-accounts'
          >
            Service accounts
          </TabsTrigger>
          <TabsTrigger value='tokens' className='px-3 text-xs' data-testid='api-access-tab-tokens'>
            Tokens
          </TabsTrigger>
          <TabsTrigger value='policy' className='px-3 text-xs' data-testid='api-access-tab-policy'>
            Policy
          </TabsTrigger>
        </TabsList>

        <TabsContent value='service-accounts'>
          <ServiceAccounts org={org} selectedId={accountId} onSelect={setAccountId} />
        </TabsContent>
        <TabsContent value='tokens'>
          <Inventory org={org} onOpenAccount={openAccount} />
        </TabsContent>
        <TabsContent value='policy'>
          <Policy org={org} />
        </TabsContent>
      </Tabs>
    </div>
  );
}
