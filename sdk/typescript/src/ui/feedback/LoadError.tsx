// Started from pokehouse-oxy `apps/supply-chain/src/components/LoadError.tsx` with its
// `useRetryFocus` (INVENTORY section 2: the reference's API plus focus handed on after Try again,
// which fixes the reference's focus-to-<body> defect; decisions 14 and 28), folding in Store Ops'
// `title`, `retryLabel` and `retrying` (`chrome/LoadError.tsx`) and Bookkeeping's default re-read
// and `noRetry` (`components/ui/LoadError.tsx`). Kept: `role="alert"` on the message, Try again
// beside it and outside the alert, `tone` and `size`. New: `retrying` is the Button's busy state
// (it keeps focus and refuses a second press; Store Ops drew it by hand), and `KitRetryText`
// is Bookkeeping's phone-row form.
//
// What a screen says when a read it depends on has failed.
//
// **`role="alert"`.** The failure arrives after the load, where a loading line was announcing
// itself (`role="status"`). A plain paragraph there is silent: a screen-reader user hears the wait
// and then nothing, at the one moment the screen has something to tell them.
//
// **Try again, beside the message.** "Try again in a moment" with nothing to press is an
// instruction the screen does not let you follow. It sits outside the alert, so the announcement
// is the sentence and not "…, Try again, button". It reads again only what failed: `onRetry` (a
// hook's own `refresh`), or, by default, the nearest `RereadBoundary`'s re-read, which mounts the
// part inside it again. Pass `noRetry` where reading again cannot change the answer; the message
// must then say what can.
//
// **Focus is handed on, never dropped** (focus/useRetryFocus.ts): from Try again to the loading
// line, then to the start of what the re-read drew, or to the next callout's Try again.

import { type ReactNode, useRef } from "react";
import { Button } from "../actions/Button";
import { cx } from "../cx";
import { useRereadAction } from "../focus/useReread";
import { kitButton, markRetry, useRetryButtonFocus } from "../focus/useRetryFocus";
import { ui } from "../recipes";
import { ArrowClockwiseIcon } from "./icons";

export interface LoadErrorProps {
  /** Reads again. Default: the nearest `RereadBoundary`'s re-read. */
  onRetry?: () => void;
  /** Reading again cannot change the answer: offer nothing to press. */
  noRetry?: boolean;
  /** The claim, on a line of its own: "Couldn't load templates". */
  title?: ReactNode;
  /** The button's word where the read again is something else: "Reload page". Default "Try
   *  again". */
  retryLabel?: string;
  /** A read again is in flight: the button says so, keeps focus and does not ask twice. */
  retrying?: boolean;
  /** `error` (the default), or `warning` for a read the screen goes on without. */
  tone?: "error" | "warning";
  /** `lg` on a touch screen: the 44px button and the message a step larger. */
  size?: "sm" | "lg";
  /** The detail: what failed, in plain words. */
  children?: ReactNode;
  className?: string;
}

export function LoadError({
  onRetry,
  noRetry = false,
  title,
  retryLabel = "Try again",
  retrying = false,
  tone = "error",
  size = "sm",
  children,
  className
}: LoadErrorProps) {
  const reread = useRereadAction();
  const retry = noRetry ? undefined : (onRetry ?? reread);
  return (
    <div className={cx("oxy-ui-load-error", className)}>
      <div
        role='alert'
        className={cx(
          ui.prose,
          ui.callout,
          ui.calloutTone[tone],
          "oxy-ui-load-error__message",
          size === "lg" && "oxy-ui-load-error__message--lg"
        )}
      >
        {title && <p className='oxy-ui-load-error__title'>{title}</p>}
        {title && children ? <div>{children}</div> : children}
      </div>
      {retry && <TryAgain size={size} label={retryLabel} retrying={retrying} onRetry={retry} />}
    </div>
  );
}

/** Try again: reads again, and passes keyboard focus on to what the re-read draws. */
function TryAgain({
  size,
  label,
  retrying,
  onRetry
}: {
  size: "sm" | "lg";
  label: string;
  retrying: boolean;
  onRetry: () => void;
}) {
  const button = useRef<HTMLButtonElement>(null);
  useRetryButtonFocus(button);
  return (
    <Button
      ref={button}
      size={size}
      icon={<ArrowClockwiseIcon />}
      busy={retrying}
      busyLabel='Trying again…'
      onClick={(e) => {
        markRetry(e.currentTarget);
        onRetry();
      }}
    >
      {label}
    </Button>
  );
}

/**
 * The words of a phone kit row that is a failed read's Try again (a `PhoneRow` with `onClick`,
 * which takes no ref): the span finds the row's button around it, so focus is handed on as from
 * Try again. The row's onClick calls `markRetry()` first.
 */
export function KitRetryText({ children }: { children: ReactNode }) {
  const span = useRef<HTMLSpanElement>(null);
  useRetryButtonFocus(span, kitButton);
  return <span ref={span}>{children}</span>;
}
