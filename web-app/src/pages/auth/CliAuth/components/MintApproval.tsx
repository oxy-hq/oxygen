import type React from "react";
import { useState } from "react";
import { KeyHint } from "@/components/ui/KeyHint";
import { Button } from "@/components/ui/shadcn/button";
import { Separator } from "@/components/ui/shadcn/separator";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useAuthorizeCli from "@/hooks/api/cliAuth/useAuthorizeCli";
import { sandboxMintErrorMessage } from "@/hooks/api/userTokens/tokenErrors";
import { useTokenOptions } from "@/hooks/api/userTokens/useUserTokens";
import { apiStatus } from "@/libs/apiError";
import { sandboxApps, sandboxLimits } from "@/libs/sandboxAgentToken";
import type { MintRequest } from "../cliAuthRequest";
import { reviewMintAsk } from "../mintReview";
import useApprovalArmed from "../useApprovalArmed";
import useApprovalKeys from "../useApprovalKeys";
import MintCaution from "./MintCaution";
import MintPage from "./MintPage";
import MintPowers from "./MintPowers";
import MintRequestSummary from "./MintRequestSummary";

interface Props {
  request: MintRequest;
  /** Who the token will act as. */
  email?: string;
  /** Handed the single-use code oxyc exchanges for the token. */
  onApproved: (code: string) => void;
  onDeclined: () => void;
  onSessionLapsed: () => void;
}

/**
 * What oxyc asks to have minted, and the one act that approves it. Mounted once the session is
 * known to be good, since the apps it resolves against are read under that session.
 *
 * The apps arrive as `<org>/<app>` references and are resolved against the apps this person may
 * mint for. Nothing is approved that doesn't resolve or is out of range: Approve stays off and
 * the value that is wrong says why, where it stands.
 *
 * Approving is a click on Approve, or the button focused and pressed: no shortcut does it, and
 * the button is off until the request has been in front of the person for a moment. Nothing on
 * the page takes focus on load.
 */
const MintApproval: React.FC<Props> = ({
  request,
  email,
  onApproved,
  onDeclined,
  onSessionLapsed
}) => {
  const options = useTokenOptions();
  const authorize = useAuthorizeCli();
  // Worded when the refusal arrives, while the app it names is still among the options.
  const [refusal, setRefusal] = useState<string | null>(null);

  const apps = sandboxApps(options.data);
  const limits = sandboxLimits(options.data);
  const review = options.data ? reviewMintAsk(request.ask, request.hostname, apps, limits) : null;
  const mint = review?.mint;
  const refused = review !== null && !mint;
  const armed = useApprovalArmed(Boolean(mint));
  const canApprove = armed && !authorize.isPending;

  const approve = () => {
    if (!mint || !armed) return;
    setRefusal(null);
    authorize.mutate(
      { code_challenge: request.codeChallenge, hostname: request.hostname, mint },
      {
        onSuccess: ({ code }) => onApproved(code),
        onError: (error) => {
          if (apiStatus(error) === 401) return onSessionLapsed();
          setRefusal(sandboxMintErrorMessage(error, apps, limits, "cli"));
        }
      }
    );
  };

  useApprovalKeys(authorize.isPending ? undefined : onDeclined);

  return (
    <MintPage
      status='confirm'
      title={
        refused ? "This request can't be approved as it stands" : "Approve a sandbox agent token?"
      }
      // The two things a person checks first: the computer asking, and who the agent acts as.
      lead={
        <>
          Asked from the computer{" "}
          <b className='break-words font-medium text-foreground' data-testid='cli-auth-hostname'>
            {request.hostname}
          </b>
          , for an agent that will act as{" "}
          <b className='break-words font-medium text-foreground' data-testid='cli-auth-approver'>
            {email ?? "you"}
          </b>
          .
        </>
      }
    >
      {review ? (
        <MintRequestSummary ask={request.ask} review={review} />
      ) : options.isError ? (
        <div
          className='mt-6 flex flex-col items-start gap-3 py-2 text-sm leading-5.5'
          data-testid='cli-auth-mint-options-error'
        >
          <p>Couldn't load the apps you can create tokens for, so the request can't be checked.</p>
          <Button variant='outline' size='sm' onClick={() => options.refetch()}>
            Try again
          </Button>
        </div>
      ) : (
        <div className='mt-6 flex flex-col gap-4 py-2' data-testid='cli-auth-mint-loading'>
          <span className='sr-only'>Checking the apps this request names</span>
          <Skeleton className='h-5 w-48' />
          <Skeleton className='h-5 w-80 max-w-full' />
          <Skeleton className='h-5 w-64' />
        </div>
      )}

      <Separator className='my-3' />
      {mint && (
        <>
          <MintPowers />
          <Separator className='my-3' />
          <MintCaution hostname={request.hostname} />
        </>
      )}
      {refused && (
        <p
          className='mt-2 font-medium text-sm leading-5.5'
          role='alert'
          data-testid='cli-auth-mint-problem'
        >
          Run <span className='font-mono'>oxyc</span> again with the request put right.
        </p>
      )}
      {refusal && (
        <p
          className='mt-4 text-destructive text-sm leading-5.5'
          role='alert'
          data-testid='cli-auth-error'
        >
          {refusal}
        </p>
      )}

      <div className='mt-6 flex justify-end gap-2'>
        <Button
          variant='outline'
          className='px-3'
          onClick={onDeclined}
          disabled={authorize.isPending}
          aria-keyshortcuts='Escape'
          data-testid='cli-auth-cancel'
        >
          Cancel
          <KeyHint className='-mr-1'>Esc</KeyHint>
        </Button>
        <Button onClick={approve} disabled={!canApprove} data-testid='cli-auth-confirm'>
          {authorize.isPending && <Spinner className='size-4' />}
          Approve
        </Button>
      </div>
    </MintPage>
  );
};

export default MintApproval;
