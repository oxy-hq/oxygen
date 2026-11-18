import { Building2 } from "lucide-react";
import type React from "react";
import { useEffect, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";
import { Separator } from "@/components/ui/shadcn/separator";
import useServiceAccountPreview from "@/hooks/api/cliAuth/useServiceAccountPreview";
import { serviceAccountMintErrorMessage, tokenErrorCode } from "@/hooks/api/userTokens/tokenErrors";
import { apiStatus } from "@/libs/apiError";
import { TokenExpiry } from "@/services/api/tokenExpiry";
import type { ServiceAccountMintPreview } from "@/types/apiToken";
import type { ServiceAccountAsk } from "../cliAuthRequest";
import { serviceAccountCommand, serviceAccountMint } from "../serviceAccountReview";
import useApprovalArmed from "../useApprovalArmed";
import useApprovalKeys from "../useApprovalKeys";
import DeviceCodeCheck from "./DeviceCodeCheck";
import MintActions from "./MintActions";
import type { ApprovalProps } from "./MintApproval";
import MintCaution from "./MintCaution";
import MintPage from "./MintPage";
import MintPending from "./MintPending";
import MintPowers from "./MintPowers";
import { Row } from "./MintRows";

/**
 * What the token may do, as the account, in the organization it belongs to. An admin's token
 * passes every org-admin route that does not ask for a browser session, member and invitation
 * management among them — said in those words, since that is what the approver hands to the code
 * that will hold it.
 */
const powers = (role: string) => ({
  can: [
    role === "admin"
      ? "Do what an admin of this organization can do through the API, over all of it"
      : "Do what a member of this organization can do through the API, over all of it",
    ...(role === "admin" ? ["Change members' roles, remove members and send invitations"] : []),
    "Be kept as a custom app's secret and used by its functions"
  ],
  cannot: [
    "Reach any other organization",
    "Create a service account, or mint a token",
    "Act as you, or carry your staff or partner access"
  ]
});

/** Who is asking and for whom: the computer, the organization, and the approver. */
const Lead: React.FC<{ hostname: string; email?: string }> = ({ hostname, email }) => (
  <>
    Asked from the computer{" "}
    <b className='break-words font-medium text-foreground' data-testid='cli-auth-hostname'>
      {hostname}
    </b>
    . You approve it as{" "}
    <b className='break-words font-medium text-foreground' data-testid='cli-auth-approver'>
      {email ?? "yourself"}
    </b>
    ; the token acts as the account, not as you.
  </>
);

/**
 * What sets this token apart from the agent tokens the same page approves: it belongs to the
 * organization, so it outlives the approver's own access. Said before the request, in the
 * page's own ink — red stays for what is refused.
 */
const OrgOwnedNotice: React.FC = () => (
  <Alert className='mt-6 border-foreground' data-testid='cli-auth-service-account-notice'>
    <Building2 aria-hidden='true' />
    <AlertTitle className='line-clamp-none'>This token belongs to the organization</AlertTitle>
    <AlertDescription>
      <p>
        It keeps working if you leave the organization. It is listed, and revoked, on the
        organization's API access page.
      </p>
    </AlertDescription>
  </Alert>
);

/** The request as the server would carry it out, a row each. */
const Summary: React.FC<{ preview: ServiceAccountMintPreview }> = ({ preview }) => {
  const { org, account, token, grant } = preview;
  return (
    <dl className='mt-6 text-sm leading-5.5' data-testid='cli-auth-service-account'>
      <Row label='Organization'>
        <div className='break-words' data-testid='cli-auth-sa-org'>
          <span className='font-medium'>{org.name}</span>{" "}
          <span className='text-muted-foreground'>{org.slug}</span>
        </div>
      </Row>
      <Row label='Account'>
        <div className='break-words' data-testid='cli-auth-sa-account'>
          <span className='font-medium'>{account.name}</span>{" "}
          <span className='text-muted-foreground'>{account.role}</span>
          <p className='mt-0.5' data-testid='cli-auth-sa-account-state'>
            {account.exists
              ? "An existing account. Approving adds a token to it and changes nothing else."
              : "A new account. Approving creates it."}
          </p>
        </div>
      </Row>
      <Row label='Token'>
        <div className='break-words font-medium' data-testid='cli-auth-mint-name'>
          {token.name}
        </div>
      </Row>
      <Row label='Lifetime'>
        <div className='flex flex-wrap gap-x-4' data-testid='cli-auth-mint-lifetime'>
          <span className='min-w-40 font-medium'>{token.expires_in_days} days</span>
          {token.expires_at && (
            <span className='text-muted-foreground'>
              until {TokenExpiry.formatDate(token.expires_at)}
            </span>
          )}
        </div>
      </Row>
      <Row label='Reaches'>
        <p className='font-medium' data-testid='cli-auth-sa-reach'>
          {grant.org_wide
            ? `All of ${org.name}, as ${grant.role_ceiling === "admin" ? "an admin" : "a member"}`
            : `Part of ${org.name}, as ${grant.role_ceiling}`}
        </p>
      </Row>
    </dl>
  );
};

/**
 * What was asked, as oxyc sent it, on a request that can't be approved: the person sees what they
 * are turning down even though the server laid nothing out.
 */
const Asked: React.FC<{ ask: ServiceAccountAsk }> = ({ ask }) => (
  <dl className='mt-6 text-sm leading-5.5' data-testid='cli-auth-sa-asked'>
    <Row label='Organization'>
      <div className='break-words font-medium'>{ask.org}</div>
    </Row>
    <Row label='Account'>
      <div className='break-words'>
        <span className='font-medium'>{ask.account}</span>{" "}
        <span className='text-muted-foreground'>{ask.role ?? "member"}</span>
      </div>
    </Row>
  </dl>
);

/**
 * What to do about a refusal. Running oxyc again fixes a request that was wrong; it does not
 * give the approver a role they lack, so that one says who can approve instead.
 */
const nextStep = (code: string | undefined): React.ReactNode => {
  switch (code) {
    case "org_admin_required":
      return "Ask one of them to approve it.";
    case "org_not_found":
      return (
        <>
          Check the organization in the command, and run <span className='font-mono'>oxyc</span>{" "}
          again.
        </>
      );
    default:
      return (
        <>
          Run <span className='font-mono'>oxyc</span> again once it is put right.
        </>
      );
  }
};

/**
 * The approval of an organization's service-account token (`oxyc tokens create
 * --org-service-account`): the account is reused, or created when the organization has none of
 * that name, and a token is minted for it. Only an owner or admin of the organization may
 * approve, and the server says so before anything is shown as approvable.
 *
 * WHAT IS SHOWN IS THE SERVER'S. The request is sent to the preview route first, which runs every
 * check the approval runs and writes nothing; the rows are its answer — the organization by
 * name, the account new or existing and its role, the token's name, its lifetime and what it
 * reaches. A request it refuses is shown refused, with its reason, and Approve never turns on.
 *
 * The sheet's rules are the other approvals': Approve arms after a second in front of the person,
 * no key approves, Escape cancels, and a device-code request waits for the code to be confirmed.
 */
const ServiceAccountApproval: React.FC<ApprovalProps<ServiceAccountAsk>> = ({
  request,
  email,
  submit,
  userCode,
  onApproved,
  onDeclined,
  onSessionLapsed
}) => {
  const [codeMatches, setCodeMatches] = useState(userCode === undefined);
  const [refusal, setRefusal] = useState<string | null>(null);
  const asked = serviceAccountMint(request.ask);
  const mint = "mint" in asked ? asked.mint : null;
  const preview = useServiceAccountPreview(request.hostname, mint);
  const sessionLapsed = apiStatus(preview.error) === 401;
  // A 4xx is the server refusing the request; anything else is a failure to read it.
  const previewRefused =
    preview.isError && !sessionLapsed && (apiStatus(preview.error) ?? 500) < 500;
  const ready = Boolean(mint && preview.data);
  const armed = useApprovalArmed(ready, preview.data);

  // The session lapsed while the page sat open: sign in again and come back here.
  useEffect(() => {
    if (sessionLapsed) onSessionLapsed();
  }, [sessionLapsed, onSessionLapsed]);

  const approve = () => {
    if (!mint || !armed || !codeMatches) return;
    setRefusal(null);
    submit.send(mint, {
      onSuccess: onApproved,
      onError: (error) => {
        if (apiStatus(error) === 401) return onSessionLapsed();
        setRefusal(serviceAccountMintErrorMessage(error));
      }
    });
  };

  useApprovalKeys(submit.pending ? undefined : onDeclined);

  const refused = "problem" in asked || previewRefused;
  const problem =
    "problem" in asked
      ? asked.problem
      : previewRefused
        ? serviceAccountMintErrorMessage(preview.error)
        : null;

  return (
    <MintPage
      status='confirm'
      title={
        refused
          ? "This request can't be approved as it stands"
          : "Approve an organization's service account token?"
      }
      lead={<Lead hostname={request.hostname} email={email} />}
    >
      {!refused && <OrgOwnedNotice />}
      {preview.data ? (
        <Summary preview={preview.data} />
      ) : refused ? (
        <Asked ask={request.ask} />
      ) : (
        <MintPending
          failed={preview.isError}
          onRetry={() => preview.refetch()}
          loadingLabel='Checking what approving this would create'
          failedText="Couldn't check what approving this would create, so it can't be approved yet."
        />
      )}

      <Separator className='my-3' />
      {preview.data && (
        <>
          <MintPowers {...powers(preview.data.account.role)} />
          <Separator className='my-3' />
          <MintCaution
            hostname={request.hostname}
            command={serviceAccountCommand(request.ask, userCode !== undefined)}
          />
          {userCode && (
            <DeviceCodeCheck
              code={userCode}
              onMatchedChange={setCodeMatches}
              disabled={submit.pending}
            />
          )}
        </>
      )}
      <MintActions
        problem={
          refused && (
            <span data-testid='cli-auth-sa-problem'>
              {problem} {nextStep(previewRefused ? tokenErrorCode(preview.error) : undefined)}
            </span>
          )
        }
        refusal={refusal}
        pending={submit.pending}
        canApprove={armed && codeMatches && !submit.pending && !refused}
        onApprove={approve}
        onCancel={onDeclined}
      />
    </MintPage>
  );
};

export default ServiceAccountApproval;
