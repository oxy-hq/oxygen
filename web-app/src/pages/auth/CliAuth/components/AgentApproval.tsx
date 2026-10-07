import type React from "react";
import { useState } from "react";
import { Separator } from "@/components/ui/shadcn/separator";
import useAuthorizeCli from "@/hooks/api/cliAuth/useAuthorizeCli";
import { agentMintErrorMessage } from "@/hooks/api/userTokens/tokenErrors";
import { useTokenOptions } from "@/hooks/api/userTokens/useUserTokens";
import { AGENT_TOKEN_POWERS, standingWords } from "@/libs/agentToken";
import { apiStatus } from "@/libs/apiError";
import { agentMint, reviewAgentAsk } from "../agentReview";
import type { AgentMintRequest } from "../cliAuthRequest";
import useApprovalArmed from "../useApprovalArmed";
import useApprovalKeys from "../useApprovalKeys";
import AgentRequestSummary from "./AgentRequestSummary";
import AgentScopeNotice from "./AgentScopeNotice";
import MintActions from "./MintActions";
import type { ApprovalProps } from "./MintApproval";
import MintCaution from "./MintCaution";
import MintLead from "./MintLead";
import MintPage from "./MintPage";
import MintPending from "./MintPending";
import MintPowers from "./MintPowers";

/**
 * The approval of an agent token (`oxyc tokens create --agent`): a personal token that reaches
 * everything the approver does, for hours. Mounted once the session is known to be good, since
 * what the person holds is read under that session.
 *
 * It is the sandbox agent approval's sheet, with the same rules: Approve arms after a second in
 * front of the person, no key approves, Escape cancels and nothing takes focus on load. What
 * differs is said before the request, because this token is far wider than the one the same
 * page usually asks about.
 *
 * STANDING IS OFF UNLESS THE PERSON TICKS IT. Asked for and held, the approver's staff or
 * partner access is a box that starts empty, so someone who approves without reading grants the
 * smaller thing. The button then says which is being approved, in its own words, and changing
 * the box counts the second again: a click already on its way to "Approve" never lands on
 * "Approve with staff access". What is sent is `standing: false` unless the box is ticked at
 * the moment of the click.
 *
 * A lifetime out of range or a name too long is refused where it stands, and oxyc is run again
 * with the request put right.
 */
const AgentApproval: React.FC<ApprovalProps<AgentMintRequest>> = ({
  request,
  email,
  onApproved,
  onDeclined,
  onSessionLapsed
}) => {
  const options = useTokenOptions();
  const authorize = useAuthorizeCli();
  const [refusal, setRefusal] = useState<string | null>(null);
  // Empty to begin with, whatever oxyc asked: including it is the approver's own act.
  const [includeStanding, setIncludeStanding] = useState(false);

  const review = options.data ? reviewAgentAsk(request.ask, request.hostname, options.data) : null;
  const mint = review ? agentMint(review, request.ask, includeStanding) : undefined;
  const refused = review !== null && !mint;
  // Exactly what would be sent. The button's words and the second it waits both follow it.
  const withStanding = mint?.standing === true;
  const armed = useApprovalArmed(Boolean(mint), withStanding);

  const approve = () => {
    if (!mint || !armed) return;
    setRefusal(null);
    authorize.mutate(
      { code_challenge: request.codeChallenge, hostname: request.hostname, mint },
      {
        onSuccess: ({ code }) => onApproved(code),
        onError: (error) => {
          if (apiStatus(error) === 401) return onSessionLapsed();
          setRefusal(agentMintErrorMessage(error));
        }
      }
    );
  };

  useApprovalKeys(authorize.isPending ? undefined : onDeclined);

  // A server that names no limits can't mint one: there is no request to lay out, only that.
  const unsupported = review?.unsupported === true;

  return (
    <MintPage
      status='confirm'
      title={refused ? "This request can't be approved as it stands" : "Approve an agent token?"}
      lead={<MintLead hostname={request.hostname} email={email} />}
    >
      {!refused && <AgentScopeNotice />}
      {review ? (
        !unsupported && (
          <AgentRequestSummary
            ask={request.ask}
            review={review}
            included={includeStanding}
            onIncludedChange={setIncludeStanding}
            disabled={authorize.isPending}
          />
        )
      ) : (
        <MintPending
          failed={options.isError}
          onRetry={() => options.refetch()}
          loadingLabel='Checking what this request may carry'
          failedText="Couldn't load what your account holds, so the request can't be checked."
        />
      )}

      <Separator className='my-3' />
      {mint && (
        <>
          <MintPowers can={AGENT_TOKEN_POWERS.can} cannot={AGENT_TOKEN_POWERS.cannot} />
          <Separator className='my-3' />
          <MintCaution
            hostname={request.hostname}
            command={`oxyc tokens create --agent${request.ask.standing ? " --standing" : ""}`}
          />
        </>
      )}
      <MintActions
        problem={
          refused &&
          (unsupported ? (
            <>
              Oxygen here can't create agent tokens yet. Run <span className='font-mono'>oxyc</span>{" "}
              again once it has been updated.
            </>
          ) : (
            <>
              Run <span className='font-mono'>oxyc</span> again with the request put right.
            </>
          ))
        }
        refusal={refusal}
        pending={authorize.isPending}
        canApprove={armed && !authorize.isPending}
        onApprove={approve}
        onCancel={onDeclined}
        approveLabel={
          withStanding && review
            ? `Approve with ${standingWords(review.standing.held)} access`
            : "Approve"
        }
      />
    </MintPage>
  );
};

export default AgentApproval;
