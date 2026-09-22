import type { ComponentProps, FormEventHandler, KeyboardEvent, Ref } from "react";
import type { UseFormRegisterReturn } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { FieldError } from "@/components/ui/shadcn/field";
import { CREW_SIGN_IN_MESSAGES, type CrewSignInFailure } from "@/hooks/auth/useFrontline";
import { cn } from "@/libs/shadcn/utils";
import type { FrontlineStaff } from "@/types/frontline";

/**
 * The longest PIN there is: the server's `PinPolicy` takes 4–8 digits
 * (`crates/auth/src/frontline.rs`), so a ninth tap has nothing to add.
 */
const MAX_PIN_DIGITS = 8;
/** Dots drawn before anything is typed — the shortest PIN there is. */
const MIN_PIN_DIGITS = 4;

const KEYS = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

interface CrewPinStepProps {
  member: FrontlineStaff;
  pin: string;
  /**
   * The form's `pin` registration, for the real input under the dots. Its
   * changes still go through `onPinChange`, like the keypad's.
   */
  pinField: UseFormRegisterReturn<"pin">;
  /** Replace the PIN — the keypad's way in, since it types into no input. */
  onPinChange: (pin: string) => void;
  onSubmit: FormEventHandler<HTMLFormElement>;
  /** "Not you?": back to the names, with nobody picked. */
  onCancel: () => void;
  busy: boolean;
  coolingDown: boolean;
  pinError?: string;
  failure: CrewSignInFailure | null;
  /** The step itself, which takes focus when it opens. */
  stepRef: Ref<HTMLElement>;
}

/**
 * The second half of crew sign-in, shown only once a name is picked: who you
 * are, your PIN as dots, a keypad big enough for a thumb, and a way back.
 *
 * One panel, placed by the screen: docked beside the names on a tablet turned
 * on its side — so tapping another name simply switches — and docked under
 * them as a sheet everywhere else. It stays in the page's flow rather than
 * floating over it, so the names above it are never hidden behind it and
 * every one of them is still in reach of a scroll.
 *
 * Focus comes to the panel, not the PIN box: focusing the box on a tablet
 * slides the device's own keyboard up over the keypad. A hardware keyboard
 * still types straight in (see `typeFromKeyboard`), and tapping the dots
 * opens the numeric soft keyboard for anyone who prefers it.
 */
const CrewPinStep = ({
  member,
  pin,
  pinField,
  onPinChange,
  onSubmit,
  onCancel,
  busy,
  coolingDown,
  pinError,
  failure,
  stepRef
}: CrewPinStepProps) => {
  const press = (digit: string) => {
    if (pin.length < MAX_PIN_DIGITS) {
      onPinChange(pin + digit);
    }
  };

  const typeFromKeyboard = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onCancel();
      return;
    }
    // The PIN box takes its own keys, and a shortcut is not a digit.
    const target = event.target as HTMLElement;
    if (target.tagName === "INPUT" || event.ctrlKey || event.metaKey || event.altKey || busy) {
      return;
    }
    if (/^[0-9]$/.test(event.key)) {
      event.preventDefault();
      press(event.key);
    } else if (event.key === "Backspace") {
      event.preventDefault();
      onPinChange(pin.slice(0, -1));
    } else if (event.key === "Enter" && target === event.currentTarget) {
      // Only from the panel itself: Enter on a key or a link is that button's.
      event.preventDefault();
      event.currentTarget.querySelector("form")?.requestSubmit();
    }
  };

  return (
    <section
      ref={stepRef}
      tabIndex={-1}
      aria-label={`Signing in as ${member.name}`}
      onKeyDown={typeFromKeyboard}
      className={cn(
        "flex max-h-full shrink-0 flex-col overflow-y-auto rounded-t-2xl border-t bg-muted/40 p-4 shadow-lg outline-none",
        "md:landscape:w-96 md:landscape:rounded-none md:landscape:border-t-0 md:landscape:border-l md:landscape:p-6 md:landscape:shadow-none"
      )}
      data-testid='login-crew-pin-step'
    >
      <form onSubmit={onSubmit} className='mx-auto flex w-full max-w-sm flex-col gap-2 md:gap-3'>
        <div className='flex flex-col'>
          <p className='text-muted-foreground text-sm'>Signing in as</p>
          <p className='font-bold text-2xl leading-tight'>{member.name}</p>
          <p className='text-muted-foreground text-sm'>{member.identifier}</p>
        </div>

        <PinDots pin={pin} pinField={pinField} onPinChange={onPinChange} disabled={busy} />
        {pinError && <FieldError className='text-center'>{pinError}</FieldError>}

        <div className='grid grid-cols-3 gap-2'>
          {KEYS.map((digit) => (
            <KeypadButton key={digit} onClick={() => press(digit)} disabled={busy}>
              <Digit>{digit}</Digit>
            </KeypadButton>
          ))}
          <KeypadButton onClick={() => onPinChange(pin.slice(0, -1))} disabled={busy}>
            Delete
          </KeypadButton>
          <KeypadButton onClick={() => press("0")} disabled={busy}>
            <Digit>0</Digit>
          </KeypadButton>
          <KeypadButton onClick={() => onPinChange("")} disabled={busy}>
            Clear
          </KeypadButton>
        </div>

        <Button
          type='submit'
          className='h-12 w-full text-base'
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

        <Button
          type='button'
          variant='link'
          onClick={onCancel}
          disabled={busy}
          className='self-center text-muted-foreground'
          data-testid='login-crew-not-you'
        >
          Not you? Choose another name
        </Button>
      </form>
    </section>
  );
};

/**
 * The PIN as dots, over the real input. The input is invisible but on top, so
 * a tap on the dots focuses it and brings up the numeric soft keyboard, and
 * anything that fills an input — a hardware keyboard, a test, a password
 * manager — still can. At least four dots; more appear past four digits.
 */
const PinDots = ({
  pin,
  pinField,
  onPinChange,
  disabled
}: {
  pin: string;
  pinField: UseFormRegisterReturn<"pin">;
  onPinChange: (pin: string) => void;
  disabled: boolean;
}) => (
  <div className='relative mx-auto flex h-12 w-full max-w-60 items-center justify-center gap-3 rounded-md focus-within:ring-2 focus-within:ring-ring'>
    {Array.from({ length: Math.max(MIN_PIN_DIGITS, pin.length) }, (_, slot) => (
      <span
        // Slots are positions, not items: slot 3 is always the fourth digit.
        // biome-ignore lint/suspicious/noArrayIndexKey: see above
        key={slot}
        aria-hidden='true'
        className={cn(
          "size-3.5 rounded-full border-2 border-muted-foreground/60",
          slot < pin.length && "border-foreground bg-foreground"
        )}
      />
    ))}
    <input
      id='crew-pin'
      type='password'
      inputMode='numeric'
      autoComplete='one-time-code'
      maxLength={MAX_PIN_DIGITS}
      aria-label='PIN'
      className='absolute inset-0 size-full cursor-pointer opacity-0'
      data-testid='login-crew-pin'
      disabled={disabled}
      {...pinField}
      // Through the same door as the keypad, digits only: a PIN has nothing else.
      onChange={(event) =>
        onPinChange(event.target.value.replace(/\D/g, "").slice(0, MAX_PIN_DIGITS))
      }
    />
  </div>
);

const KeypadButton = ({
  className,
  ...props
}: Omit<ComponentProps<typeof Button>, "type" | "variant">) => (
  <Button type='button' variant='outline' className={cn("h-12 md:h-14", className)} {...props} />
);

/**
 * A key's digit, sized on a span of its own: `<Button>` carries `.t-button`,
 * whose unlayered font-size beats a size utility on the button itself.
 */
const Digit = ({ children }: { children: string }) => (
  <span className='font-normal text-2xl'>{children}</span>
);

export default CrewPinStep;
