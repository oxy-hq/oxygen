import { Tablet } from "lucide-react";
import { type FormEventHandler, type ReactNode, useEffect, useRef, useState } from "react";
import { type UseFormRegisterReturn, useForm } from "react-hook-form";
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
import CrewPinStep from "./CrewPinStep";
import CrewRosterPicker, { CrewRosterSkeleton } from "./CrewRosterPicker";
import { nobodyRosteredHere } from "./crewRoster";
import { readRecentCrew, rememberCrewSignIn } from "./recentCrew";

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
  /** The manager's way to account sign-in, kept to the top corner of the screen. */
  adminSignIn?: ReactNode;
}

/** How the crew on this kiosk say who they are. */
type KioskEntry = "pick" | "type" | "nobody";

/** A kiosk speaks to whoever is standing at it, not to an account holder. */
const KIOSK_SUBTITLE: Record<KioskEntry, string> = {
  pick: "Tap your name and enter your PIN",
  type: "Enter your ID and PIN",
  nobody: "Crew sign-in isn't ready on this tablet yet"
};

/** The device's name tag: which kiosk this is, and whose. */
const KioskLine = ({ device }: { device: BoundKioskDevice }) => (
  <p className='flex items-center gap-1.5 text-muted-foreground text-xs'>
    <Tablet className='size-3.5 shrink-0' aria-hidden='true' />
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
    className='mx-auto flex max-w-sm flex-col gap-1 py-10 text-center'
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
 * Crew sign-in on an enrolled kiosk — the whole screen, because on a kiosk the
 * screen belongs to the crew. Tap a name (or type an ID when a kiosk with no
 * place has no roster), enter a PIN, land in the app. A store's tablet with
 * nobody on its roster says so instead of offering a box nobody could get in
 * through (`nobodyRosteredHere`). The session rides the cookie the server
 * sets, so success is a full navigation — never `AuthContext.login`: the PIN
 * response has no user object and a worker is not a platform user here.
 */
const CrewSignIn = ({
  device,
  staff,
  isRosterLoading,
  isRosterError = false,
  returnTo,
  adminSignIn
}: CrewSignInProps) => {
  const hasRoster = staff.length > 0;
  const [signedInAs, setSignedInAs] = useState<string | null>(null);
  const [redirecting, setRedirecting] = useState(false);
  const [cooldownUntil, setCooldownUntil] = useState<number | null>(null);
  // Read once: it only changes when somebody signs in, and then this page is
  // on its way somewhere else.
  const [recent] = useState(() => readRecentCrew(device));
  const screenRef = useRef<HTMLDivElement>(null);
  const stepRef = useRef<HTMLElement>(null);
  /** The tile the PIN step was opened from, for "Not you?" to hand focus back to. */
  const pickedFrom = useRef<HTMLElement | null>(null);
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
  const pin = watch("pin");
  const selectedStaff = hasRoster
    ? staff.find((member) => member.identifier === selectedIdentifier)
    : undefined;

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

  // Into the PIN step whenever it opens on a name (or switches to another).
  const stepFor = selectedStaff?.identifier;
  useEffect(() => {
    if (stepFor) {
      stepRef.current?.focus({ preventScroll: true });
    }
  }, [stepFor]);

  const isBusy = login.isPending || redirecting;
  const isCoolingDown = cooldownUntil !== null;
  let failure: CrewSignInFailure | null = null;
  if (login.error) {
    failure = classifyCrewSignInError(login.error);
  } else if (isCoolingDown) {
    failure = "rate_limited";
  }

  /** Back to the PIN after a failed try: the step on the board, the box otherwise. */
  const focusPinEntry = () => {
    if (stepRef.current) {
      stepRef.current.focus({ preventScroll: true });
    } else {
      setFocus("pin");
    }
  };

  const pickStaff = (identifier: string) => {
    const tapped = document.activeElement;
    pickedFrom.current =
      tapped instanceof HTMLElement && tapped.dataset.identifier === identifier ? tapped : null;
    login.reset();
    // A half-typed PIN is the last person's, not this one's.
    resetField("pin");
    setValue("identifier", identifier, { shouldValidate: true });
  };

  /**
   * Every way a PIN changes on the board — keypad, hardware keyboard, the soft
   * keyboard over the dots — comes through here, so they behave alike. A shown
   * PIN error is re-checked as the PIN changes (so it clears once fixed); no
   * new one appears mid-typing, since a PIN is short until it is finished.
   */
  const typePin = (next: string) => {
    if (login.isError) {
      login.reset();
    }
    setValue("pin", next, { shouldValidate: Boolean(errors.pin) });
  };

  /**
   * "Not you?": nobody picked, and focus back on the name that was — the tile
   * it was opened from, or (after a touch, which focuses nothing) that name's
   * first tile on the board.
   */
  const chooseAnotherName = () => {
    const tile = pickedFrom.current?.isConnected
      ? pickedFrom.current
      : [...(screenRef.current?.querySelectorAll<HTMLElement>("[data-identifier]") ?? [])].find(
          (el) => el.dataset.identifier === selectedIdentifier
        );
    login.reset();
    resetField("pin");
    resetField("identifier");
    tile?.focus();
  };

  const onSubmit = (data: CrewFormData) => {
    const identifier = data.identifier.trim();
    login.mutate(
      { org: device.org, identifier, pin: data.pin },
      {
        onSuccess: async ({ name }) => {
          // Only a sign-in that worked puts a name at the top of the board.
          rememberCrewSignIn(device, identifier);
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
          focusPinEntry();
        }
      }
    );
  };

  const rosterState = { isLoading: isRosterLoading, isError: isRosterError };
  const nobodyHere = nobodyRosteredHere(device, staff, rosterState);
  // While the roster is still loading, assume the common case (there is one)
  // so the subtitle doesn't flip mid-read.
  let entry: KioskEntry = "type";
  if (nobodyHere) {
    entry = "nobody";
  } else if (isRosterLoading || hasRoster) {
    entry = "pick";
  }

  let body: ReactNode;
  if (signedInAs) {
    body = (
      <div className='flex flex-col items-center gap-1 py-10 text-center'>
        <p className='font-semibold text-lg'>Signed in as {signedInAs}.</p>
        <p className='text-muted-foreground text-sm'>
          This kiosk has no app to open — ask your manager.
        </p>
      </div>
    );
  } else if (nobodyHere) {
    body = <NobodyRosteredHere device={device} />;
  } else if (isRosterLoading) {
    body = <CrewRosterSkeleton />;
  } else if (hasRoster) {
    body = (
      <>
        <CrewRosterPicker
          staff={staff}
          recent={recent}
          placeName={device.location?.name || device.orgName}
          selected={selectedIdentifier}
          onSelect={pickStaff}
          disabled={isBusy}
        />
        {errors.identifier && <FieldError>{errors.identifier.message}</FieldError>}
      </>
    );
  } else {
    body = (
      <TypedIdForm
        onSubmit={handleSubmit(onSubmit)}
        identifierField={identifierField}
        pinField={pinField}
        identifierError={errors.identifier?.message}
        pinError={errors.pin?.message}
        busy={isBusy}
        coolingDown={isCoolingDown}
        failure={failure}
      />
    );
  }

  return (
    <div
      ref={screenRef}
      className='flex h-full w-full flex-col md:landscape:flex-row'
      data-testid='login-crew'
    >
      {/* The names take whatever height is left. When the PIN sheet leaves
          less than the title and the recent row need (a phone), this column
          scrolls rather than clipping them. */}
      <div className='flex min-h-0 min-w-0 flex-1 flex-col gap-4 overflow-y-auto p-4 md:p-6'>
        <header className='flex shrink-0 items-start justify-between gap-4'>
          <div className='flex min-w-0 flex-col gap-1'>
            <h1 className='font-bold text-2xl'>Who's on shift?</h1>
            <p className='text-muted-foreground text-sm'>{KIOSK_SUBTITLE[entry]}</p>
            <KioskLine device={device} />
          </div>
          {adminSignIn}
        </header>
        {body}
      </div>

      {selectedStaff && !signedInAs && (
        <CrewPinStep
          member={selectedStaff}
          pin={pin}
          pinField={pinField}
          onPinChange={typePin}
          onSubmit={handleSubmit(onSubmit)}
          onCancel={chooseAnotherName}
          busy={isBusy}
          coolingDown={isCoolingDown}
          pinError={errors.pin?.message}
          failure={failure}
          stepRef={stepRef}
        />
      )}
    </div>
  );
};

/**
 * The way in on a kiosk with no place and no roster (or a roster that failed
 * to load): an ID and a PIN, typed. Rare, and it needs a keyboard anyway, so it
 * stays the plain form it always was.
 */
const TypedIdForm = ({
  onSubmit,
  identifierField,
  pinField,
  identifierError,
  pinError,
  busy,
  coolingDown,
  failure
}: {
  onSubmit: FormEventHandler<HTMLFormElement>;
  identifierField: UseFormRegisterReturn<"identifier">;
  pinField: UseFormRegisterReturn<"pin">;
  identifierError?: string;
  pinError?: string;
  busy: boolean;
  coolingDown: boolean;
  failure: CrewSignInFailure | null;
}) => (
  <form onSubmit={onSubmit} className='mx-auto flex w-full max-w-xs flex-col gap-4 py-6'>
    <div className='grid gap-2'>
      <Label htmlFor='crew-identifier'>Your ID</Label>
      <Input
        id='crew-identifier'
        autoComplete='username'
        autoCapitalize='none'
        autoCorrect='off'
        spellCheck={false}
        className='h-12 text-base md:text-base'
        data-testid='login-crew-identifier'
        disabled={busy}
        {...identifierField}
      />
      {identifierError && <FieldError>{identifierError}</FieldError>}
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
        disabled={busy}
        {...pinField}
      />
      {pinError && <FieldError>{pinError}</FieldError>}
    </div>

    <Button
      type='submit'
      className='h-12 w-full'
      disabled={busy || coolingDown}
      data-testid='login-crew-submit'
    >
      {busy ? "Signing in…" : "Sign in"}
    </Button>

    {failure && (
      <FieldError data-testid='login-crew-error' className='text-center'>
        {CREW_SIGN_IN_MESSAGES[failure]}
      </FieldError>
    )}
  </form>
);

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
