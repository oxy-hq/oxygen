import type React from "react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useAuthorizeCli from "@/hooks/api/cliAuth/useAuthorizeCli";
import { apiStatus } from "@/libs/apiError";
import { codeCallbackUrl, loginUrl, type PkceRequest, returnToUrl } from "../cliAuthRequest";
import { type CliSession, leaveTo, readCliSession } from "../cliSession";
import CliAuthCard from "./CliAuthCard";

type Step = "checking" | "confirm" | "redirecting" | "declined";

/**
 * The PKCE half of `oxyc login`. Nothing is authorized, and nothing leaves the page, until the
 * person reads which computer is asking and clicks: the link that opened this page could have
 * come from anywhere, and the hostname is the only thing that says where the token will live.
 *
 * On confirm the session is traded for a single-use code, and only that code goes to oxyc's
 * loopback listener. oxyc exchanges it, with a verifier only it holds, for its own token.
 */
const PkceAuthorize: React.FC<{ request: PkceRequest }> = ({ request }) => {
  const [step, setStep] = useState<Step>("checking");
  const [session, setSession] = useState<CliSession | null>(null);
  const authorize = useAuthorizeCli();

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

  const confirm = () =>
    authorize.mutate(
      { code_challenge: request.codeChallenge, hostname: request.hostname },
      {
        onSuccess: ({ code }) => {
          setStep("redirecting");
          leaveTo(codeCallbackUrl(request, code));
        },
        onError: (error) => {
          // The session lapsed while the page sat open: sign in again and come back here.
          if (apiStatus(error) === 401) leaveTo(loginHref);
        }
      }
    );

  if (step === "checking") {
    return (
      <CliAuthCard
        status='working'
        title='Checking your session…'
        description='Making sure you are signed in before oxyc asks for access.'
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
        description='Nothing was authorized. oxyc will stop waiting on its own; you can close this tab.'
      />
    );
  }

  const failed = authorize.isError && apiStatus(authorize.error) !== 401;
  return (
    <CliAuthCard
      status='confirm'
      title={
        <>
          Sign in to oxyc on{" "}
          <span className='break-all' data-testid='cli-auth-hostname'>
            {request.hostname}
          </span>
          ?
        </>
      }
      description={
        <>
          oxyc on that computer will act as {session?.email ?? "you"}, with everything you can
          reach, for 90 days. It replaces any earlier oxyc login from the same computer.
        </>
      }
    >
      <div className='flex flex-col gap-3'>
        {failed && (
          <p
            className='text-center text-destructive text-sm'
            role='alert'
            data-testid='cli-auth-error'
          >
            Couldn't authorize oxyc. Try again, or run <code>oxyc login</code> again for a new link.
          </p>
        )}
        <div className='flex justify-center gap-2'>
          <Button
            variant='outline'
            onClick={() => setStep("declined")}
            disabled={authorize.isPending}
            data-testid='cli-auth-cancel'
          >
            Cancel
          </Button>
          <Button onClick={confirm} disabled={authorize.isPending} data-testid='cli-auth-confirm'>
            {authorize.isPending && <Spinner className='size-4' />}
            Authorize oxyc
          </Button>
        </div>
        <p className='text-center text-muted-foreground text-xs'>
          Continue only if you just ran <code>oxyc login</code> on {request.hostname}.
        </p>
      </div>
    </CliAuthCard>
  );
};

export default PkceAuthorize;
