import { CopyBlock } from "@/components/CopyBlock";
import type { Organization } from "@/types/organization";
import { blankStartCommand, bootstrapScript } from "./bootstrapScript";

/**
 * What an Oxy engineer runs to set this org up. Setting an org up left the web app:
 * it is `oxyc` (the customer's repo) plus the API that connects it
 * (`POST /orgs/{org_id}/onboarding/{github,new,demo}`). This only prints the commands,
 * filled in with this org's id and a repo name, so nobody has to look the id up.
 *
 * Shown only to someone the server says may create here (`may_provision_workspace`):
 * an owner or admin of the org who is Oxy staff with reach over it. Anyone else
 * would be refused by those endpoints.
 */
export function BootstrapCommands({ org }: { org: Organization }) {
  return (
    <div
      className='w-full space-y-3 rounded-lg border bg-card p-5 text-left'
      data-testid='org-setup-pending-bootstrap'
    >
      <div className='space-y-1'>
        <h2 className='font-medium text-sm'>Set up {org.name} from your terminal</h2>
        <p className='text-muted-foreground text-xs'>
          Only Oxy staff see this. Create the customer's repo, then connect it to {org.name}.
        </p>
      </div>
      <CopyBlock
        text={bootstrapScript(org)}
        label={`Copy the commands that set up ${org.name}`}
        testId='org-setup-pending-bootstrap-script'
        nowrap
      />
      <p className='text-muted-foreground text-xs'>To start from a blank one instead:</p>
      <CopyBlock
        text={blankStartCommand(org)}
        label='Copy the blank start command'
        testId='org-setup-pending-bootstrap-blank'
        nowrap
      />
    </div>
  );
}
