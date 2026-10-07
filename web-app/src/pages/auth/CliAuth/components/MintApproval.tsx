import type React from "react";
import { useState } from "react";
import { Separator } from "@/components/ui/shadcn/separator";
import useAuthorizeCli from "@/hooks/api/cliAuth/useAuthorizeCli";
import { sandboxMintErrorMessage } from "@/hooks/api/userTokens/tokenErrors";
import { useTokenOptions } from "@/hooks/api/userTokens/useUserTokens";
import { apiStatus } from "@/libs/apiError";
import { sandboxAgentPowers, sandboxApps, sandboxLimits } from "@/libs/sandboxAgentToken";
import type { MintRequest } from "../cliAuthRequest";
import { reviewMintAsk } from "../mintReview";
import useApprovalArmed from "../useApprovalArmed";
import useApprovalKeys from "../useApprovalKeys";
import MintActions from "./MintActions";
import MintCaution from "./MintCaution";
import MintLead from "./MintLead";
import MintPage from "./MintPage";
import MintPending from "./MintPending";
import MintPowers from "./MintPowers";
import MintRequestSummary from "./MintRequestSummary";

const POWERS = sandboxAgentPowers("these apps");

export interface ApprovalProps<Request> {
  request: Request;
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
const MintApproval: React.FC<ApprovalProps<MintRequest>> = ({
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
      lead={<MintLead hostname={request.hostname} email={email} />}
    >
      {review ? (
        <MintRequestSummary ask={request.ask} review={review} />
      ) : (
        <MintPending
          failed={options.isError}
          onRetry={() => options.refetch()}
          loadingLabel='Checking the apps this request names'
          failedText="Couldn't load the apps you can create tokens for, so the request can't be checked."
        />
      )}

      <Separator className='my-3' />
      {mint && (
        <>
          <MintPowers can={POWERS.can} cannot={POWERS.cannot} />
          <Separator className='my-3' />
          <MintCaution hostname={request.hostname} command='oxyc tokens create --sandbox-agent' />
        </>
      )}
      <MintActions
        problem={
          refused && (
            <>
              Run <span className='font-mono'>oxyc</span> again with the request put right.
            </>
          )
        }
        refusal={refusal}
        pending={authorize.isPending}
        canApprove={armed && !authorize.isPending}
        onApprove={approve}
        onCancel={onDeclined}
      />
    </MintPage>
  );
};

export default MintApproval;
