import { Tablet } from "lucide-react";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { FieldError } from "@/components/ui/shadcn/field";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  CREW_SIGN_IN_MESSAGES,
  type CrewSignInFailure,
  classifyCrewSignInError,
  resolveCrewDestination,
  useFrontlineLogin
} from "@/hooks/auth/useFrontline";
import type { BoundKioskDevice, FrontlineStaff } from "@/types/frontline";
import CrewRosterPicker, { CrewRosterSkeleton } from "./CrewRosterPicker";
import { nobodyRosteredHere } from "./crewRoster";

type CrewFormData = {
  identifier: string;
  pin: string;
};

/** How long "Sign in" stays down after the org answers 429. */
const RATE_LIMIT_COOLDOWN_MS = 60_000;

interface CrewSignInProps {
  device: BoundKioskDevice;
  staff: FrontlineStaff[];
  isRosterLoading: boolean;
  /**
   * The roster read failed. Not the same as a roster that came back empty: a
   * failure says nothing about who works here, so the ID box stays.
   */
  isRosterError?: boolean;
  /**
   * The `return_to` the app sent the worker here with. Wins over the app the
   * kiosk was enrolled for; both are validated server-side before any redirect.
   */
  returnTo?: string;
}

/** The device's name tag: which kiosk this is, and whose. */
const KioskLine = ({ device }: { device: BoundKioskDevice }) => (
  <p className='flex items-center justify-center gap-1.5 text-muted-foreground text-xs'>
    <Tablet className='size-3.5' aria-hidden='true' />
    <span>
      {[device.device, device.location?.name, device.orgName].filter(Boolean).join(" · ")}
    </span>
  </p>
);

/**
 * A store's tablet with nobody on its roster. Words for the crew member standing
 * at it: nothing they type will work, and who can fix that.
 */
const NobodyRosteredHere = ({ device }: { device: BoundKioskDevice }) => (
  <div
    role='status'
    className='flex flex-col gap-1 text-center'
    data-testid='login-crew-nobody-here'
  >
    <p className='font-medium'>
      Nobody is set up to sign in at {device.location?.name || "this store"} yet.
    </p>
    <p className='text-muted-foreground text-sm'>
      A manager has to add the crew to this store before anyone can sign in here. Let your manager
      know.
    </p>
  </div>
);

/**
 * Crew sign-in on an enrolled kiosk. Tap a name (or type an ID when a kiosk
 * with no place has no roster), enter a PIN, land in the app. A store's tablet
 * with nobody on its roster says so instead of offering a box nobody could get
 * in through (`nobodyRosteredHere`). The session rides the cookie the server
 * sets, so success is a full navigation — never `AuthContext.login`: the PIN
 * response has no user object and a worker is not a platform user here.
 */
const CrewSignIn = ({
  device,
  staff,
  isRosterLoading,
  isRosterError = false,
  returnTo
}: CrewSignInProps) => {
  const hasRoster = staff.length > 0;
  const [signedInAs, setSignedInAs] = useState<string | null>(null);
  const [redirecting, setRedirecting] = useState(false);
  const [cooldownUntil, setCooldownUntil] = useState<number | null>(null);
  const login = useFrontlineLogin();

  const {
    register,
    handleSubmit,
    setValue,
    setFocus,
    resetField,
    watch,
    formState: { errors }
  } = useForm<CrewFormData>({ defaultValues: { identifier: "", pin: "" } });

  // Registered whether or not an input is on screen: with a roster the value
  // arrives from a tile tap via `setValue`, and the same rule still runs on
  // submit — so "nobody picked yet" is a form error like any other.
  const identifierField = register("identifier", {
    validate: (value) =>
      value.trim().length > 0 || (hasRoster ? "Tap your name first." : "Enter your ID.")
  });
  const pinField = register("pin", {
    required: "Enter your PIN.",
    minLength: { value: 4, message: "Your PIN is at least 4 digits." },
    // A fresh attempt starts clean; the old "didn't match" goes with the old PIN.
    onChange: () => {
      if (login.isError) {
        login.reset();
      }
    }
  });
  const selectedIdentifier = watch("identifier");
  const selectedStaff = staff.find((member) => member.identifier === selectedIdentifier);

  useEffect(() => {
    if (cooldownUntil === null) {
      return;
    }
    const timer = window.setTimeout(
      () => setCooldownUntil(null),
      Math.max(cooldownUntil - Date.now(), 0)
    );
    return () => window.clearTimeout(timer);
  }, [cooldownUntil]);

  const isBusy = login.isPending || redirecting;
  const isCoolingDown = cooldownUntil !== null;
  let failure: CrewSignInFailure | null = null;
  if (login.error) {
    failure = classifyCrewSignInError(login.error);
  } else if (isCoolingDown) {
    failure = "rate_limited";
  }

  const pickStaff = (identifier: string) => {
    login.reset();
    setValue("identifier", identifier, { shouldValidate: true });
    setFocus("pin");
  };

  const onSubmit = (data: CrewFormData) => {
    login.mutate(
      { org: device.org, identifier: data.identifier.trim(), pin: data.pin },
      {
        onSuccess: async ({ name }) => {
          setRedirecting(true);
          const destination = await resolveCrewDestination(returnTo, device.returnTo);
          if (destination) {
            window.location.href = destination;
            return;
          }
          setRedirecting(false);
          setSignedInAs(name);
        },
        onError: (error) => {
          resetField("pin");
          if (classifyCrewSignInError(error) === "rate_limited") {
            setCooldownUntil(Date.now() + RATE_LIMIT_COOLDOWN_MS);
          }
          setFocus("pin");
        }
      }
    );
  };

  if (signedInAs) {
    return (
      <div data-testid='login-crew' className='flex flex-col items-center gap-1 text-center'>
        <p className='font-semibold text-lg'>Signed in as {signedInAs}.</p>
        <p className='text-muted-foreground text-sm'>
          This kiosk has no app to open — ask your manager.
        </p>
      </div>
    );
  }

  if (nobodyRosteredHere(device, staff, { isLoading: isRosterLoading, isError: isRosterError })) {
    return (
      <div className='flex flex-col gap-4' data-testid='login-crew'>
        <KioskLine device={device} />
        <NobodyRosteredHere device={device} />
      </div>
    );
  }

  return (
    <form
      onSubmit={handleSubmit(onSubmit)}
      className='flex flex-col gap-4'
      data-testid='login-crew'
    >
      <KioskLine device={device} />

      <div className='grid gap-2'>
        {isRosterLoading && <CrewRosterSkeleton />}
        {!isRosterLoading && hasRoster && (
          <CrewRosterPicker
            staff={staff}
            selected={selectedIdentifier}
            onSelect={pickStaff}
            disabled={isBusy}
          />
        )}
        {!isRosterLoading && !hasRoster && (
          <>
            <Label htmlFor='crew-identifier'>Your ID</Label>
            <Input
              id='crew-identifier'
              autoComplete='username'
              autoCapitalize='none'
              autoCorrect='off'
              spellCheck={false}
              className='h-12 text-base md:text-base'
              data-testid='login-crew-identifier'
              disabled={isBusy}
              {...identifierField}
            />
          </>
        )}
        {selectedStaff && (
          <p className='text-muted-foreground text-xs'>
            Signing in as <span className='font-medium text-foreground'>{selectedStaff.name}</span>{" "}
            · {selectedStaff.identifier}
          </p>
        )}
        {errors.identifier && <FieldError>{errors.identifier.message}</FieldError>}
      </div>

      <div className='grid gap-2'>
        <Label htmlFor='crew-pin'>PIN</Label>
        <Input
          id='crew-pin'
          type='password'
          inputMode='numeric'
          autoComplete='one-time-code'
          className='h-12 text-lg tracking-widest md:text-lg'
          data-testid='login-crew-pin'
          disabled={isBusy}
          {...pinField}
        />
        {errors.pin && <FieldError>{errors.pin.message}</FieldError>}
      </div>

      <Button
        type='submit'
        className='h-12 w-full'
        disabled={isBusy || isCoolingDown}
        data-testid='login-crew-submit'
      >
        {isBusy ? "Signing in…" : "Sign in"}
      </Button>

      {failure && (
        <FieldError data-testid='login-crew-error' className='text-center'>
          {CREW_SIGN_IN_MESSAGES[failure]}
        </FieldError>
      )}
    </form>
  );
};

/**
 * For a worker who reached the login page from a custom app on a browser that
 * isn't an enrolled kiosk. There is nothing here they can do; say so quietly.
 */
export const CrewSignInHint = () => (
  <p data-testid='login-crew-hint' className='text-center text-muted-foreground text-xs'>
    Crew member? Crew sign-in works on an enrolled kiosk — ask your manager.
  </p>
);

export default CrewSignIn;
