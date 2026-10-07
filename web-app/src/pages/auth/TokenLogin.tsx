import { isAxiosError } from "axios";
import type React from "react";
import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { AuthCard, AuthLayout } from "@/components/AuthLayout";
import { Button } from "@/components/ui/shadcn/button";
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
  NEW_LINK_COMMAND,
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
    return (
      <AuthLayout>
        <FailureCard failure={failure} onBack={() => navigate(ROUTES.AUTH.LOGIN)} />
      </AuthLayout>
    );
  }

  if (awaitingConfirm) {
    return (
      <AuthLayout>
        <ReplaceSessionCard
          onConfirm={() => {
            setAwaitingConfirm(false);
            if (link.ticket) redeem(link.ticket);
          }}
          onCancel={() => navigate(ROUTES.ROOT, { replace: true })}
        />
      </AuthLayout>
    );
  }

  return (
    <AuthLayout>
      <div
        className='flex items-center justify-center gap-3 py-10 text-muted-foreground text-sm'
        data-testid='token-login-pending'
      >
        <Spinner />
        Signing in…
      </div>
    </AuthLayout>
  );
};

interface FailureCardProps {
  failure: TokenLoginFailure;
  onBack: () => void;
}

/** Why the link did not sign anyone in, the command that mints another, and the way back. */
const FailureCard: React.FC<FailureCardProps> = ({ failure, onBack }) => {
  const { title, description } = describeTokenLoginFailure(failure);
  return (
    <AuthCard title={title} description={description} testId='token-login-error'>
      <code className='rounded-md bg-muted px-3 py-2 font-mono text-sm'>{NEW_LINK_COMMAND}</code>
      <Button variant='outline' onClick={onBack} className='w-full'>
        Back to sign in
      </Button>
    </AuthCard>
  );
};

interface ReplaceSessionCardProps {
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * Asks the person already signed in before the link takes their session. The
 * copy is short, but it must say who they would become: that is the one fact
 * that stops someone who was sent a link they did not ask for.
 */
const ReplaceSessionCard: React.FC<ReplaceSessionCardProps> = ({ onConfirm, onCancel }) => {
  const signedInAs = storedUserLabel();
  return (
    <AuthCard
      title='Replace session?'
      description={`Signed in${signedInAs ? ` as ${signedInAs}` : ""}. Continuing signs this browser in as whoever made the link — only continue if that was you.`}
      testId='token-login-replace-session'
    >
      <div className='flex gap-2'>
        <Button
          variant='outline'
          className='flex-1'
          onClick={onCancel}
          data-testid='token-login-cancel'
        >
          Cancel
        </Button>
        <Button className='flex-1' onClick={onConfirm} data-testid='token-login-confirm'>
          Continue
        </Button>
      </div>
    </AuthCard>
  );
};

export default TokenLogin;
