import { Mail } from "lucide-react";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Link, useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger
} from "@/components/ui/shadcn/dialog";
import { FieldError } from "@/components/ui/shadcn/field";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useAuth } from "@/contexts/AuthContext";
import { returnToPointsAtCustomApp, useFrontlineRoster } from "@/hooks/auth/useFrontline";
import { useRequestMagicLink } from "@/hooks/auth/useMagicLink";
import ROUTES from "@/libs/utils/routes";
import type { BoundKioskDevice } from "@/types/frontline";
import CrewSignIn, { CrewSignInHint } from "./CrewSignIn";
import LoginWithGitHubButton from "./LoginWithGitHubButton";
import LoginWithGoogleButton from "./LoginWithGoogleButton";
import LoginWithOktaButton from "./LoginWithOktaButton";
import { devLoginNext, kioskAdminReturnTo } from "./signInDestination";

type MagicLinkFormData = {
  email: string;
};

const isRateLimited = (error: unknown) =>
  (error as { response?: { status?: number } })?.response?.status === 429;

const getRateLimitMessage = (error: unknown) =>
  (error as { response?: { data?: { message?: string } } })?.response?.data?.message ??
  "Too many sign-in attempts. Please try again later.";

type View = "form" | "sent";

const MagicLinkSection = ({ returnTo: destination }: { returnTo?: string }) => {
  const [view, setView] = useState<View>("form");
  const [submittedEmail, setSubmittedEmail] = useState("");
  const [searchParams] = useSearchParams();
  // Forwarded into the magic-link request. The server allowlists the value
  // before embedding it into the email; the verify-callback page validates
  // again before performing the redirect.
  const returnTo = destination ?? searchParams.get("return_to") ?? undefined;
  const { mutateAsync: requestMagicLink, isPending } = useRequestMagicLink();

  const {
    register,
    handleSubmit,
    formState: { errors }
  } = useForm<MagicLinkFormData>();

  const onSubmit = async (data: MagicLinkFormData) => {
    try {
      await requestMagicLink({ email: data.email, return_to: returnTo });
      setSubmittedEmail(data.email);
      setView("sent");
    } catch (error) {
      if (isRateLimited(error)) {
        toast.error(getRateLimitMessage(error));
      } else {
        toast.error("Something went wrong. Please try again.");
      }
    }
  };

  const handleResend = async () => {
    try {
      await requestMagicLink({ email: submittedEmail, return_to: returnTo });
      toast.success("Sign-in link resent.");
    } catch (error) {
      if (isRateLimited(error)) {
        toast.error(getRateLimitMessage(error));
      } else {
        toast.error("Something went wrong. Please try again.");
      }
    }
  };

  if (view === "sent") {
    return (
      <div className='flex flex-col items-center gap-4 text-center'>
        <div className='flex h-14 w-14 items-center justify-center rounded-full bg-primary/10'>
          <Mail className='h-7 w-7 text-primary' />
        </div>
        <div className='flex flex-col gap-1'>
          <h2 className='font-semibold text-lg'>Check your inbox</h2>
          <p className='text-muted-foreground text-sm'>
            We sent a sign-in link to{" "}
            <span className='font-medium text-foreground'>{submittedEmail}</span>. It expires in 15
            minutes.
          </p>
        </div>
        <div className='flex flex-col gap-2 text-sm'>
          <button
            type='button'
            onClick={handleResend}
            disabled={isPending}
            className='text-primary underline-offset-4 hover:underline disabled:opacity-50'
          >
            {isPending ? <Spinner /> : "Didn't receive it? Resend"}
          </button>
          <button
            type='button'
            onClick={() => setView("form")}
            className='text-muted-foreground underline-offset-4 hover:underline'
          >
            Use a different email
          </button>
        </div>
      </div>
    );
  }

  return (
    <form onSubmit={handleSubmit(onSubmit)} className='flex flex-col gap-3'>
      <div className='grid gap-2'>
        <Label htmlFor='magic-email'>Email</Label>
        <Input
          id='magic-email'
          type='email'
          placeholder='you@example.com'
          {...register("email", {
            required: "Email is required",
            pattern: {
              value: /^[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}$/i,
              message: "Invalid email address"
            }
          })}
          disabled={isPending}
        />
        {errors.email && <FieldError>{errors.email.message}</FieldError>}
      </div>
      <Button type='submit' className='w-full' disabled={isPending}>
        {isPending ? "Sending link…" : "Continue with email"}
      </Button>
    </form>
  );
};

const Divider = ({ label }: { label: string }) => (
  <div className='relative text-center text-sm after:absolute after:inset-0 after:top-1/2 after:z-0 after:flex after:items-center after:border-border after:border-t'>
    <span className='relative z-10 bg-background px-2 text-muted-foreground'>{label}</span>
  </div>
);

/**
 * Only rendered when the server reports the bypass reachable *by this caller*
 * (`authConfig.dev_login` — see the field's docstring; an inferred allow-list
 * is loopback-only, so this is absent off-box). It exists so an agent driving
 * the browser has something to click — the login page is where automation
 * lands, and every other button here leads off to a provider or an inbox.
 */
const DevSignInSection = ({ next }: { next?: string }) => (
  <Link
    to={next ? `${ROUTES.AUTH.DEV_LOGIN}?next=${encodeURIComponent(next)}` : ROUTES.AUTH.DEV_LOGIN}
    className='w-full'
    data-testid='login-dev-signin'
  >
    <Button type='button' variant='outline' className='w-full'>
      Dev sign-in (no password)
    </Button>
  </Link>
);

/**
 * Everything an account holder signs in with: magic link, OAuth, and the dev
 * bypass. `returnTo` is where every one of them lands; absent, each follows the
 * login URL's own `return_to`, as the ordinary login page always has.
 */
const AccountSignIn = ({ returnTo }: { returnTo?: string }) => {
  const { authConfig } = useAuth();
  const hasOAuth = Boolean(authConfig.google || authConfig.okta || authConfig.github);
  const hasMagicLink = Boolean(authConfig.magic_link);

  return (
    <>
      {hasMagicLink && <MagicLinkSection returnTo={returnTo} />}

      {hasOAuth && hasMagicLink && <Divider label='or' />}

      {authConfig.github && (
        <LoginWithGitHubButton
          disabled={false}
          clientId={authConfig.github.client_id}
          returnTo={returnTo}
        />
      )}
      {authConfig.google && (
        <LoginWithGoogleButton
          disabled={false}
          clientId={authConfig.google.client_id}
          returnTo={returnTo}
        />
      )}
      {authConfig.okta && (
        <LoginWithOktaButton
          disabled={false}
          clientId={authConfig.okta.client_id}
          domain={authConfig.okta.domain}
          returnTo={returnTo}
        />
      )}

      {authConfig.dev_login && (
        <>
          <Divider label='dev only' />
          <DevSignInSection next={devLoginNext(returnTo)} />
        </>
      )}
    </>
  );
};

/**
 * On a kiosk the screen belongs to the crew. Account sign-in stays one tap away
 * for the manager setting the tablet up — in the top corner, out of the way of
 * the names — but never sits under the PIN pad where a worker could wander into
 * an email form.
 *
 * Every provider here lands on `/kiosk`, never on the login URL's `return_to`
 * (see `kioskAdminReturnTo`): the admin came to manage the tablet, and on a
 * kiosk that `return_to` is the crew's app.
 */
const AdminSignInDialog = () => (
  <Dialog>
    <DialogTrigger asChild>
      <Button
        type='button'
        variant='link'
        size='sm'
        className='shrink-0 px-0 text-muted-foreground'
        data-testid='login-admin-signin'
      >
        Sign in as an admin
      </Button>
    </DialogTrigger>
    <DialogContent className='sm:max-w-sm' data-testid='login-admin-dialog'>
      <DialogHeader>
        <DialogTitle>Sign in as an admin</DialogTitle>
        <DialogDescription>
          Use your Oxygen account. Crew sign in with their PIN on this screen.
        </DialogDescription>
      </DialogHeader>
      <div className='flex flex-col gap-4'>
        <AccountSignIn returnTo={kioskAdminReturnTo()} />
      </div>
    </DialogContent>
  </Dialog>
);

/**
 * An enrolled kiosk's login page: the store's crew on the whole screen, and the
 * admin's way in tucked into its top corner.
 */
export const KioskLogin = ({ device }: { device: BoundKioskDevice }) => {
  const { authConfig } = useAuth();
  const [searchParams] = useSearchParams();
  // Crew sign-in's first-choice destination (validated server-side before any
  // redirect).
  const returnTo = searchParams.get("return_to") ?? undefined;
  const {
    data: staff = [],
    isLoading: isRosterLoading,
    isError: isRosterError
  } = useFrontlineRoster(device.org);

  const hasAccountSignIn = Boolean(
    authConfig.magic_link ||
      authConfig.google ||
      authConfig.okta ||
      authConfig.github ||
      authConfig.dev_login
  );

  return (
    <CrewSignIn
      device={device}
      staff={staff}
      isRosterLoading={isRosterLoading}
      isRosterError={isRosterError}
      returnTo={returnTo}
      adminSignIn={hasAccountSignIn ? <AdminSignInDialog /> : undefined}
    />
  );
};

/** Every other browser's login page: account sign-in. */
const LoginForm = () => {
  const [searchParams] = useSearchParams();
  // The magic-link section reads the same param on its own.
  const returnTo = searchParams.get("return_to") ?? undefined;

  return (
    <div className='flex flex-col gap-6'>
      <div className='flex flex-col items-center gap-2 text-center'>
        <h1 className='font-bold text-2xl'>Welcome back</h1>
        <p className='text-muted-foreground text-sm'>Sign in to your account to continue</p>
      </div>

      <div className='flex flex-col gap-4'>
        <AccountSignIn />
        {returnToPointsAtCustomApp(returnTo) && <CrewSignInHint />}
      </div>
    </div>
  );
};

export default LoginForm;
