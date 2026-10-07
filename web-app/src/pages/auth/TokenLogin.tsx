import { isAxiosError } from "axios";
import { KeyRound, XCircle } from "lucide-react";
import type React from "react";
import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle
} from "@/components/ui/shadcn/card";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useTokenLogin } from "@/hooks/auth/useTokenLogin";
import {
  isAuthTokenExpired,
  isStoredTokenSession,
  storedUserLabel
} from "@/libs/utils/authStorage";
import ROUTES from "@/libs/utils/routes";
import {
  classifyTokenLoginFailure,
  describeTokenLoginFailure,
  type TokenLoginFailure
} from "./describeTokenLoginFailure";
import { readTokenLoginLink, stripFragment } from "./tokenLoginLink";

/**
 * A person is signed in on this browser: the stored session is live, and it
 * was not itself opened with an API token.
 *
 * Such a session is replaced only on a click. A link someone else minted must
 * not be able to silently swap a person's session for theirs (login CSRF) —
 * everything the victim then did would land in the attacker's account. An
 * agent's browser only ever holds a token session or nothing, so it never sees
 * the card and still signs in with one navigation.
 */
const holdsLiveHumanSession = (): boolean => !isAuthTokenExpired() && !isStoredTokenSession();

/**
 * `/token-login#ticket=…` — `/dev-login` for a deployed environment.
 *
 * An automation agent holding a personal API token asks the server for a
 * one-time link (`oxyc login-link`) and navigates a browser to it. This page
 * redeems the ticket on mount and redirects, leaving the browser signed in as
 * the token's owner. As with `/dev-login`, the UI below only shows while that
 * round-trip is in flight or when it fails — plus one case dev-login has no
 * reason for: a person is already signed in here, and has to agree first.
 *
 * Fragment params — the fragment, not the query string, so that none of them
 * reaches a server log or a `Referer`:
 *   `ticket`    the one-time ticket (required; works once, lasts 5 minutes)
 *   `next`      same-origin path to land on, e.g. `#ticket=…&next=/ide`
 *   `return_to` cross-origin destination, validated server-side
 */
const TokenLogin: React.FC = () => {
  const navigate = useNavigate();
  // Read once and kept in state: the effect below takes the fragment out of
  // the address bar, so a later render would find nothing there to read.
  const [link] = useState(() => readTokenLoginLink(window.location.hash));
  const [awaitingConfirm, setAwaitingConfirm] = useState(holdsLiveHumanSession);
  // `setFailure` is a stable useState setter, so it stays correct even when the
  // callback that closes over it came from the discarded StrictMode pass.
  const [failure, setFailure] = useState<TokenLoginFailure | null>(link.ticket ? null : "link");
  const { mutate: redeem } = useTokenLogin({
    returnTo: link.returnTo,
    next: link.next,
    onFailure: (error) =>
      setFailure(
        classifyTokenLoginFailure(isAxiosError(error) ? error.response?.status : undefined)
      )
  });
  // Latch, not `status === "idle"`: StrictMode runs mount → cleanup → mount in
  // one commit, before React Query has flushed `status` to "pending", so both
  // passes would read "idle" and redeem twice. A ticket works once — the
  // second redeem is refused, and its error card would race the first one's
  // redirect. A ref is set synchronously, so the second pass sees it.
  const fired = useRef(false);

  useEffect(() => {
    if (fired.current) return;
    fired.current = true;
    // Before anything else, and whatever happens next: the ticket must not
    // linger in the URL to be copied, bookmarked or screenshotted.
    stripFragment();
    if (link.ticket && !awaitingConfirm) redeem(link.ticket);
  }, [redeem, link, awaitingConfirm]);

  if (failure) {
    return <FailureCard failure={failure} onBack={() => navigate(ROUTES.AUTH.LOGIN)} />;
  }

  if (awaitingConfirm) {
    return (
      <ReplaceSessionCard
        onConfirm={() => {
          setAwaitingConfirm(false);
          if (link.ticket) redeem(link.ticket);
        }}
        onCancel={() => navigate(ROUTES.ROOT, { replace: true })}
      />
    );
  }

  return (
    <div
      className='flex min-h-screen w-full items-center justify-center bg-background p-4'
      data-testid='token-login-pending'
    >
      <div className='flex flex-col items-center gap-3'>
        <Spinner />
        <p className='text-muted-foreground text-sm'>Signing in…</p>
      </div>
    </div>
  );
};

interface NoticeProps {
  testId: string;
  icon: React.ReactNode;
  title: string;
  description: string;
  children: React.ReactNode;
}

/** The centered card both the refusal and the confirmation are shown in. */
const Notice: React.FC<NoticeProps> = ({ testId, icon, title, description, children }) => (
  <div
    className='flex min-h-screen w-full items-center justify-center bg-background p-4'
    data-testid={testId}
  >
    <Card className='w-full max-w-md'>
      <CardHeader className='text-center'>
        <div className='mb-4 flex justify-center'>{icon}</div>
        <CardTitle className='text-2xl'>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent>{children}</CardContent>
    </Card>
  </div>
);

interface FailureCardProps {
  failure: TokenLoginFailure;
  onBack: () => void;
}

/** Why the link did not sign anyone in, and the way back to an ordinary login. */
const FailureCard: React.FC<FailureCardProps> = ({ failure, onBack }) => {
  const { title, description } = describeTokenLoginFailure(failure);
  return (
    <Notice
      testId='token-login-error'
      icon={<XCircle className='h-12 w-12 text-destructive' />}
      title={title}
      description={description}
    >
      <Button onClick={onBack} className='w-full'>
        Back to login
      </Button>
    </Notice>
  );
};

interface ReplaceSessionCardProps {
  onConfirm: () => void;
  onCancel: () => void;
}

/** Asks the person already signed in before the link takes their session. */
const ReplaceSessionCard: React.FC<ReplaceSessionCardProps> = ({ onConfirm, onCancel }) => {
  const signedInAs = storedUserLabel();
  return (
    <Notice
      testId='token-login-replace-session'
      icon={<KeyRound className='h-12 w-12 text-muted-foreground' />}
      title='Replace your session?'
      description={`You're already signed in${signedInAs ? ` as ${signedInAs}` : ""}. Continue to replace this session with an API-token session?`}
    >
      <div className='flex flex-col gap-3'>
        <div className='flex justify-center gap-2'>
          <Button variant='outline' onClick={onCancel} data-testid='token-login-cancel'>
            Cancel
          </Button>
          <Button onClick={onConfirm} data-testid='token-login-confirm'>
            Continue
          </Button>
        </div>
        <p className='text-center text-muted-foreground text-xs'>
          Continue only if you asked for this link. It signs this browser in as whoever minted it.
        </p>
      </div>
    </Notice>
  );
};

export default TokenLogin;
