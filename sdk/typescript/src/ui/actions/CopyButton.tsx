// New in the SDK (BEYOND-GALLERY row 11: `clipboard.writeText` in 8 files / 6 repos, each inline,
// none tested, none announcing its result). Built from the two copies that do the most: the
// fallback is the reference's (oxy-hq/reference-app@3caad1f `src/views/PatternTokens.tsx:75-102`:
// a refused Clipboard API falls back to `execCommand("copy")` on a hidden textarea), the polite
// status beside the button and the words for a refusal are Store Ops'
// (pokehouse-oxy:apps/store-ops/src/views/settings/TabletDialog.tsx:191-229). New: focus returns
// to the button after the fallback's textarea took it, the confirmation clears itself, and the
// look is the SDK's Button.
//
// Copy a value, then say how it went, in words, where a screen reader hears it without focus
// moving: the status beside the button is a polite live region, there from the first render so
// its change is announced. The icon turns to a check while "Copied" shows. When the browser
// refuses both ways (no permission, an insecure page, no focus), it says so and the value stays
// on screen for the person to copy by hand; it is never reported anywhere.

import { useEffect, useRef, useState } from "react";
import { cx } from "../cx";
import { CheckIcon } from "../icons";
import { ui } from "../recipes";
import { Button, type ButtonProps } from "./Button";
import { CopyIcon } from "./icons";

export interface CopyButtonProps
  extends Omit<
    ButtonProps,
    "onClick" | "icon" | "busy" | "busyLabel" | "inert" | "onCopy" | "value"
  > {
  /** The text to copy, or a reader called at the press (a value that changes). */
  value: string | (() => string);
  /** What the status says once the text is on the clipboard (default "Copied"). */
  copiedLabel?: string;
  /** What it says when the browser refuses (default "Couldn't copy. Select it and copy it
   *  yourself."). Stays until the next press. */
  failedLabel?: string;
  /** How long "Copied" stays, in ms (default 2000). */
  resetAfter?: number;
  /** Told how the copy went: true on the clipboard, false refused. */
  onCopy?: (ok: boolean) => void;
}

type Outcome = "idle" | "copied" | "failed";

/** The old way, for a page the Clipboard API refuses: a selected hidden textarea and
 *  `execCommand("copy")`. Focus goes back where it was, since the textarea took it. */
function copyByCommand(text: string): boolean {
  if (typeof document.execCommand !== "function") return false;
  const had = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.setAttribute("aria-hidden", "true");
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.focus({ preventScroll: true });
  area.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  area.remove();
  had?.focus({ preventScroll: true });
  return ok;
}

/** Copy to the clipboard, falling back when the Clipboard API is missing or refuses. */
async function copyText(text: string): Promise<boolean> {
  try {
    if (!navigator.clipboard?.writeText) throw new Error("no Clipboard API");
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return copyByCommand(text);
  }
}

export function CopyButton({
  value,
  copiedLabel = "Copied",
  failedLabel = "Couldn't copy. Select it and copy it yourself.",
  resetAfter = 2000,
  onCopy,
  className,
  children = "Copy",
  ...rest
}: CopyButtonProps) {
  const [outcome, setOutcome] = useState<Outcome>("idle");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  const copy = async () => {
    clearTimeout(timer.current);
    const ok = await copyText(typeof value === "function" ? value() : value);
    setOutcome(ok ? "copied" : "failed");
    if (ok) timer.current = setTimeout(() => setOutcome("idle"), resetAfter);
    onCopy?.(ok);
  };

  const message = outcome === "copied" ? copiedLabel : outcome === "failed" ? failedLabel : "";
  return (
    <span className={cx("oxy-ui-copy-button", className)}>
      <Button
        {...rest}
        icon={outcome === "copied" ? <CheckIcon /> : <CopyIcon />}
        onClick={() => void copy()}
      >
        {children}
      </Button>
      {/* Polite: read without moving focus off the button. */}
      <span role='status' className={cx(ui.note, "oxy-ui-copy-button__status")}>
        {message}
      </span>
    </span>
  );
}
