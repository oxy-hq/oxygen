import type React from "react";
import { useCallback, useEffect, useState } from "react";
import { KeyHint } from "@/components/ui/KeyHint";
import { Button } from "@/components/ui/shadcn/button";
import { type LegacyRequest, loginUrl, returnToUrl, tokenCallbackUrl } from "../cliAuthRequest";
import { type CliSession, leaveTo, readCliSession } from "../cliSession";
import useApprovalArmed from "../useApprovalArmed";
import useApprovalKeys from "../useApprovalKeys";
import CliAuthCard from "./CliAuthCard";

type Step = "checking" | "confirm" | "redirecting" | "declined";

/**
 * The flow an oxyc that predates PKCE expects: `?port=<loopback>&state=<nonce>` and nothing else.
 * What it is handed is the browser's own session token, which reaches everything the person can
 * and can't be revoked.
 *
 * So the page asks first. Anything on the computer that can open a URL and listen on a port can
 * start this flow, an AI agent with a shell included, and the `state` nonce only ties the
 * callback to whoever opened the page: it says nothing about a person meaning to sign in. Nothing
 * goes to the loopback until the person presses the button. The button is off until the request
 * has been in front of them for a moment, so a click already on its way when the page opened
 * lands on nothing, and no key approves; Escape cancels. A request carries no hostname, so the
 * card names none.
 *
 * Signed out, bounce through the normal login and return here. Once approved, the handoff is
 * what it was before PKCE existed, so an older oxyc still completes its login, one click later.
 */
const LegacyHandoff: React.FC<{ request: LegacyRequest }> = ({ request }) => {
  const [step, setStep] = useState<Step>("checking");
  const [session, setSession] = useState<CliSession | null>(null);

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

  const asking = step === "confirm";
  const armed = useApprovalArmed(asking);
  const decline = useCallback(() => setStep("declined"), []);
  useApprovalKeys(asking ? decline : undefined);

  const handOver = () => {
    if (!armed || !session) return;
    setStep("redirecting");
    // Top-level navigation to an http loopback is allowed even from an https origin (loopback
    // is exempt from mixed-content blocking), so this works on both local and cloud.
    leaveTo(tokenCallbackUrl(request, session.token));
  };

  if (step === "checking") {
    return (
      <CliAuthCard
        status='working'
        title='Checking your session…'
        description='Making sure you are signed in. Nothing is sent until you say so.'
      />
    );
  }
  if (step === "redirecting") {
    return (
      <CliAuthCard
        status='done'
        title='Login complete'
        description='Returning to your terminal. You can close this tab.'
      />
    );
  }
  if (step === "declined") {
    return (
      <CliAuthCard
        status='error'
        title='Login cancelled'
        description='Nothing was sent. A login waiting in your terminal stops on its own; you can close this tab.'
      />
    );
  }

  return (
    <CliAuthCard
      status='confirm'
      title={
        <span className='block text-balance'>
          A program on this computer is asking for your sign-in
        </span>
      }
      description={
        <>
          Whatever receives it can do everything you can
          {session?.email && (
            <>
              {" "}
              as{" "}
              <b
                className='break-words font-medium text-foreground'
                data-testid='cli-auth-approver'
              >
                {session.email}
              </b>
            </>
          )}
          , for as long as the sign-in lasts.
        </>
      }
    >
      <div className='flex flex-col gap-4'>
        <p className='text-balance text-center text-sm' data-testid='cli-auth-caution'>
          Continue only if you just ran <code className='whitespace-nowrap'>oxyc login</code> or{" "}
          <code className='whitespace-nowrap'>oxy login</code> on this computer.
        </p>
        <div className='flex justify-center gap-2'>
          <Button
            variant='outline'
            className='px-3'
            onClick={decline}
            aria-keyshortcuts='Escape'
            data-testid='cli-auth-cancel'
          >
            Cancel
            <KeyHint className='-mr-1'>Esc</KeyHint>
          </Button>
          <Button onClick={handOver} disabled={!armed} data-testid='cli-auth-confirm'>
            Hand over sign-in
          </Button>
        </div>
        <p
          className='text-balance text-center text-muted-foreground text-xs'
          data-testid='cli-auth-upgrade'
        >
          A current oxyc asks for a token you can revoke instead. To get it, run{" "}
          <code className='whitespace-nowrap'>npm i -g @oxy-hq/cli@latest</code>.
        </p>
      </div>
    </CliAuthCard>
  );
};

export default LegacyHandoff;
