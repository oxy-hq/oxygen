import type React from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import type { TokenOptions } from "@/types/apiToken";
import {
  type AccessAction,
  type AccessDraft,
  draftProblem,
  partnerOrgsWithoutStanding
} from "../../../accessDraft";
import Caution, { listNames } from "./Caution";
import OrgGrantCard from "./OrgGrantCard";

interface Props {
  draft: AccessDraft;
  dispatch: React.Dispatch<AccessAction>;
  /** `GET /user/token-options`; `undefined` while loading or after a failure. */
  options?: TokenOptions;
  loading: boolean;
  failed: boolean;
  onRetry: () => void;
}

/** The org → workspace list behind "Selected workspaces". */
const GrantPicker: React.FC<Props> = ({ draft, dispatch, options, loading, failed, onRetry }) => {
  if (loading) {
    return (
      <div className='flex flex-col gap-2' data-testid='account-token-options-loading'>
        <Skeleton className='h-9 w-full' />
        <Skeleton className='h-20 w-full' />
      </div>
    );
  }
  if (failed || !options) {
    return (
      <div
        className='flex flex-col items-start gap-2 rounded-md border border-destructive/30 p-3 text-xs'
        data-testid='account-token-options-error'
      >
        <p className='font-medium'>Couldn't load your organizations</p>
        <Button
          type='button'
          variant='outline'
          size='sm'
          className='h-7 px-2 text-xs'
          onClick={onRetry}
        >
          Try again
        </Button>
      </div>
    );
  }
  if (options.orgs.length === 0) {
    return (
      <p className='rounded-md border p-3 text-muted-foreground text-xs'>
        You aren't in an organization yet, so there are no workspaces to select.
      </p>
    );
  }
  const partnerOrgs = partnerOrgsWithoutStanding(draft, options);
  return (
    <div className='flex flex-col gap-2'>
      <div
        className='flex max-h-64 flex-col gap-2 overflow-y-auto'
        data-testid='account-token-grant-picker'
      >
        {options.orgs.map((org) => (
          <OrgGrantCard
            key={org.org_id}
            org={org}
            draft={draft.orgs[org.org_id]}
            revoked={draft.revoked}
            dispatch={dispatch}
          />
        ))}
      </div>
      <p className='text-muted-foreground text-xs'>
        {draftProblem(draft) ??
          "Read can view. Write can run and edit. Admin can manage the workspace. Full does whatever you can."}
      </p>
      {partnerOrgs.length > 0 && (
        <Caution testId='account-token-partner-caution'>
          You reach {listNames(partnerOrgs)} as a partner. Tick "Include partner access" below, or
          the token won't work there.
        </Caution>
      )}
    </div>
  );
};

export default GrantPicker;
