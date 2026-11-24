import { useEffect, useRef } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { oauthStateToken, useGoogleAuth, validateGoogleState } from "@/hooks/auth/useGoogleAuth";
import ROUTES from "@/libs/utils/routes";

const GoogleCallback = () => {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const googleAuthMutation = useGoogleAuth();
  const authAttempted = useRef(false);

  useEffect(() => {
    if (authAttempted.current) return;
    authAttempted.current = true;

    const code = searchParams.get("code");
    const state = searchParams.get("state");
    const error = searchParams.get("error");

    // Check for OAuth errors first
    if (error) {
      // Google's own refusal, usually `access_denied` because the person
      // cancelled the consent screen. Not a defect here, and `sentry.ts`
      // captures every `console.error` as a Sentry error event
      // (captureConsoleIntegration), so this one stays a warning: visible in
      // the browser console, out of the error view. The CSRF branch below
      // stays `console.error` — a failed state check is worth an event.
      console.warn("Google OAuth error:", error);
      navigate(`${ROUTES.AUTH.LOGIN}?error=oauth_failed`);
      return;
    }

    // Validate CSRF state token (critical security check)
    if (!validateGoogleState(state)) {
      console.error("CSRF validation failed - potential attack detected");
      navigate(`${ROUTES.AUTH.LOGIN}?error=csrf_validation_failed`);
      return;
    }

    // Proceed with authentication if we have a valid code. validateGoogleState
    // already confirmed `state` is non-null above (early returns otherwise).
    if (code && state) {
      // `state` may carry an appended instance origin (bounce proxy); the
      // backend only wants the bare CSRF JWT. CSRF was validated above on the
      // full value, so stripping here is safe.
      googleAuthMutation.mutate({ code, state: oauthStateToken(state) });
    } else {
      navigate(`${ROUTES.AUTH.LOGIN}?error=no_code`);
    }
  }, [searchParams, navigate, googleAuthMutation]);

  return (
    <div className='flex h-full w-full flex-col items-center justify-center gap-5'>
      <Spinner className='size-6' />
      <p>Completing Google authentication...</p>
    </div>
  );
};

export default GoogleCallback;
