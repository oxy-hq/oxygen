import type React from "react";
import { useEffect, useState } from "react";
import useAuthorizeCli from "@/hooks/api/cliAuth/useAuthorizeCli";
import { codeCallbackUrl, loginUrl, returnToUrl, type TokenRequest } from "../cliAuthRequest";
import { type CliSession, leaveTo, readCliSession } from "../cliSession";
import AgentApproval from "./AgentApproval";
import MintApproval, { type ApprovalSubmit } from "./MintApproval";
import MintPage from "./MintPage";
import ServiceAccountApproval from "./ServiceAccountApproval";

type Step = "checking" | "confirm" | "redirecting" | "declined";

/**
 * The browser half of `oxyc tokens create --sandbox-agent`, `--agent` and
 * `--org-service-account`.
 * A token is minted only in a browser session, so oxyc opens this page and waits. Nothing is
 * authorized, and nothing leaves the page, until the person reads what is asked for and
 * approves: the link that opened this page could have come from anywhere.
 *
 * On approval the session is traded for a single-use code, as for a login, and only that code
 * goes to oxyc's loopback listener. oxyc exchanges it for the token, which is never in a URL.
 *
 * Every state is the same sheet: the page keeps its shape from the first check to the outcome.
 * The kinds of token differ only in the approval itself.
 */
const MintAuthorize: React.FC<{ request: TokenRequest }> = ({ request }) => {
  const [step, setStep] = useState<Step>("checking");
  const [session, setSession] = useState<CliSession | null>(null);
  const authorize = useAuthorizeCli();

  // Carries what was asked for, so signing in comes back to this approval and not to a login.
  const loginHref = loginUrl(returnToUrl(window.location.origin, request));

  useEffect(() => {
    let cancelled = false;
    readCliSession()
      .then((found) => {
        if (cancelled) return;
        if (!found) return leaveTo(loginHref);
        setSession(found);
        setStep("confirm");
      })
      .catch(() => {
        if (!cancelled) leaveTo(loginHref);
      });
    return () => {
      cancelled = true;
    };
  }, [loginHref]);

  if (step === "checking") {
    return (
      <MintPage
        status='working'
        title='Checking your session…'
        lead='Making sure you are signed in before oxyc asks for a token.'
      />
    );
  }
  if (step === "redirecting") {
    return (
      <MintPage
        status='done'
        title='Token approved'
        lead='Returning to your terminal, where oxyc prints the token once. You can close this tab.'
      />
    );
  }
  if (step === "declined") {
    return (
      <MintPage
        status='error'
        title='Request declined'
        lead='No token was created. oxyc will stop waiting on its own; you can close this tab.'
      />
    );
  }

  // The approval is traded for a code bound to oxyc's challenge, for the loopback.
  const submit: ApprovalSubmit = {
    pending: authorize.isPending,
    send: (mint, { onSuccess, onError }) =>
      authorize.mutate(
        { code_challenge: request.codeChallenge, hostname: request.hostname, mint },
        { onSuccess: ({ code }) => onSuccess(code), onError }
      )
  };
  const approval = {
    email: session?.email,
    submit,
    onApproved: (code: string | null) => {
      setStep("redirecting");
      if (code) leaveTo(codeCallbackUrl(request, code));
    },
    onDeclined: () => setStep("declined"),
    // The session lapsed while the page sat open: sign in again and come back here.
    onSessionLapsed: () => leaveTo(loginHref)
  };
  switch (request.kind) {
    case "agent_mint":
      return <AgentApproval request={request} {...approval} />;
    case "service_account_mint":
      return <ServiceAccountApproval request={request} {...approval} />;
    case "mint":
      return <MintApproval request={request} {...approval} />;
  }
};

export default MintAuthorize;
