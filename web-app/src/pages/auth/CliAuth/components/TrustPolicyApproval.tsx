import { Workflow } from "lucide-react";
import type React from "react";
import { useEffect, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";
import { Separator } from "@/components/ui/shadcn/separator";
import useTrustPolicyPreview from "@/hooks/api/cliAuth/useTrustPolicyPreview";
import { serviceAccountMintErrorMessage, tokenErrorCode } from "@/hooks/api/userTokens/tokenErrors";
import { apiStatus } from "@/libs/apiError";
import type { TrustPolicyMintPreview } from "@/types/apiToken";
import type { TrustPolicyAsk } from "../cliAuthRequest";
import { trustPolicyCommand, trustPolicyMint } from "../trustPolicyReview";
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

/** What a run the policy trusts may do, and what it may not. */
const powers = (app: string) => ({
  can: [`Publish ${app} as the service account, from the run shown, with no stored secret`],
  cannot: [
    "Do anything else in the organization",
    "Publish from another repository, workflow or environment",
    "Act as you, or as anyone"
  ]
});

/** Who is asking, and that nothing is handed to them. */
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
    . No token is created.
  </>
);

/**
 * What a trust policy is, said before the request: it lets runs of a workflow act as the
 * account, every time, until the policy is removed. In the page's own ink — red stays for what
 * is refused.
 */
const TrustNotice: React.FC = () => (
  <Alert className='mt-6 border-foreground' data-testid='cli-auth-trust-policy-notice'>
    <Workflow aria-hidden='true' />
    <AlertTitle className='line-clamp-none'>
      Runs of this GitHub Actions workflow will publish as the account
    </AlertTitle>
    <AlertDescription>
      <p>
        Each run gets a fifteen-minute credential of its own, until the policy is disabled or
        deleted under Settings → Organization → API tokens → Service accounts → the account →
        Trusted access.
      </p>
    </AlertDescription>
  </Alert>
);

const Summary: React.FC<{ preview: TrustPolicyMintPreview }> = ({ preview }) => {
  const { org, account, app, policy } = preview;
  return (
    <dl className='mt-6 text-sm leading-5.5' data-testid='cli-auth-trust-policy'>
      <Row label='Organization'>
        <div className='break-words' data-testid='cli-auth-tp-org'>
          <span className='font-medium'>{org.name}</span>{" "}
          <span className='text-muted-foreground'>{org.slug}</span>
        </div>
      </Row>
      <Row label='Account'>
        <div className='break-words' data-testid='cli-auth-tp-account'>
          <span className='font-medium'>{account.name}</span>{" "}
          <span className='text-muted-foreground'>{account.role}</span>
          <p className='mt-0.5' data-testid='cli-auth-tp-account-state'>
            {account.exists
              ? "An existing account. Approving changes nothing about it."
              : "A new account. Approving creates it."}
          </p>
        </div>
      </Row>
      <Row label='Repository'>
        <div className='break-words font-medium font-mono' data-testid='cli-auth-tp-repository'>
          {policy.repository}
        </div>
      </Row>
      <Row label='Workflow'>
        <div className='break-words font-mono' data-testid='cli-auth-tp-workflow'>
          {policy.workflow_path}
        </div>
      </Row>
      <Row label='Environment'>
        <div className='break-words' data-testid='cli-auth-tp-environment'>
          {policy.environment ? (
            <span className='font-mono'>{policy.environment}</span>
          ) : (
            <span className='text-muted-foreground'>Any, including none</span>
          )}
        </div>
      </Row>
      <Row label='Grant'>
        <div data-testid='cli-auth-tp-grant'>
          <p className='font-medium'>
            Publish {app.name}{" "}
            <span className='font-normal text-muted-foreground'>
              {org.slug}/{app.slug}
            </span>
          </p>
          <p className='mt-0.5' data-testid='cli-auth-tp-policy-state'>
            {policy.exists
              ? "Already trusted: approving writes nothing."
              : "And nothing else. Approving adds this trust policy."}
          </p>
        </div>
      </Row>
    </dl>
  );
};

/** What was asked, as oxyc sent it, on a request that can't be approved. */
const Asked: React.FC<{ ask: TrustPolicyAsk }> = ({ ask }) => (
  <dl className='mt-6 text-sm leading-5.5' data-testid='cli-auth-tp-asked'>
    <Row label='Organization'>
      <div className='break-words font-medium'>{ask.org}</div>
    </Row>
    <Row label='Account'>
      <div className='break-words font-medium'>{ask.account}</div>
    </Row>
    <Row label='Repository'>
      <div className='break-words font-mono'>{ask.repository}</div>
    </Row>
    <Row label='Workflow'>
      <div className='break-words font-mono'>{ask.workflow}</div>
    </Row>
  </dl>
);

/** What to do about a refusal: re-running fixes a request that was wrong, not a missing role. */
const nextStep = (code: string | undefined): React.ReactNode => {
  switch (code) {
    case "org_admin_required":
      return "Ask one of them to approve it.";
    case "repository_unresolved":
      return (
        <>
          Add the trust policy under Settings → Organization → API tokens → Service accounts → the
          account → Trusted access instead, where you enter the repository's ids yourself.
        </>
      );
    default:
      return (
        <>
          Run <span className='font-mono'>oxyc init-ci</span> again once it is put right.
        </>
      );
  }
};

/**
 * The approval of a trust policy (`oxyc init-ci`): one GitHub Actions workflow may publish one
 * app as an organization's service account, the account created first when asked. Only an owner
 * or admin of the organization may approve, and nothing is minted — the deployment writes the
 * account and the policy under this session and tells oxyc what it wrote.
 *
 * WHAT IS SHOWN IS THE SERVER'S: the rows are the answer of the preview route, which runs every
 * check the approval runs and writes nothing — the repository as GitHub names it, the account new
 * or existing, the app by name. A request it refuses is shown refused with its reason, and Approve
 * never arms. The sheet's other rules are every approval's.
 */
const TrustPolicyApproval: React.FC<ApprovalProps<TrustPolicyAsk>> = ({
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
  const mint = trustPolicyMint(request.ask);
  const preview = useTrustPolicyPreview(mint);
  const sessionLapsed = apiStatus(preview.error) === 401;
  const previewRefused =
    preview.isError && !sessionLapsed && (apiStatus(preview.error) ?? 500) < 500;
  const armed = useApprovalArmed(Boolean(preview.data), preview.data);

  // The session lapsed while the page sat open: sign in again and come back here.
  useEffect(() => {
    if (sessionLapsed) onSessionLapsed();
  }, [sessionLapsed, onSessionLapsed]);

  const approve = () => {
    if (!armed || !codeMatches) return;
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

  const refused = previewRefused;
  const problem = previewRefused ? serviceAccountMintErrorMessage(preview.error) : null;
  const appLabel = preview.data ? `${preview.data.org.slug}/${preview.data.app.slug}` : "";

  return (
    <MintPage
      status='confirm'
      title={
        refused
          ? "This request can't be approved as it stands"
          : "Let a GitHub Actions workflow publish this app?"
      }
      lead={<Lead hostname={request.hostname} email={email} />}
    >
      {!refused && <TrustNotice />}
      {preview.data ? (
        <Summary preview={preview.data} />
      ) : refused ? (
        <Asked ask={request.ask} />
      ) : (
        <MintPending
          failed={preview.isError}
          onRetry={() => preview.refetch()}
          loadingLabel='Checking what approving this would register'
          failedText="Couldn't check what approving this would register, so it can't be approved yet."
        />
      )}

      <Separator className='my-3' />
      {preview.data && (
        <>
          <MintPowers {...powers(appLabel)} />
          <Separator className='my-3' />
          <MintCaution
            hostname={request.hostname}
            command={trustPolicyCommand(preview.data.org.slug, preview.data.app.slug)}
            ranBy='you or someone you work with'
          />
          {userCode && (
            <DeviceCodeCheck
              code={userCode}
              onMatchedChange={setCodeMatches}
              disabled={submit.pending}
              caution='Approve only a request you expected. No token is created: whoever started it is told only what was written.'
            />
          )}
        </>
      )}
      <MintActions
        problem={
          refused && (
            <span data-testid='cli-auth-tp-problem'>
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

export default TrustPolicyApproval;
