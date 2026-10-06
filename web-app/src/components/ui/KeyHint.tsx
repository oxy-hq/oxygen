import { Command, CornerDownLeft } from "lucide-react";
import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import { isApplePlatform } from "@/libs/submitChord";

interface Props {
  /** `onPrimary` sits inside a filled button, where the plain fill would vanish. */
  tone?: "plain" | "onPrimary";
  className?: string;
  "data-testid"?: string;
}

/**
 * A key drawn beside the control it presses. Decoration: the control itself carries
 * `aria-keyshortcuts`, so this is hidden from a screen reader and stays out of a button's name.
 */
export const KeyHint: React.FC<React.PropsWithChildren<Props>> = ({
  tone = "plain",
  className,
  children,
  "data-testid": testId
}) => (
  <span
    aria-hidden='true'
    data-slot='key-hint'
    data-testid={testId}
    className={cn(
      "inline-flex h-5 min-w-5 shrink-0 items-center justify-center gap-px rounded-sm px-1 font-medium font-sans text-xs leading-none",
      tone === "onPrimary"
        ? "bg-primary-foreground/15 text-primary-foreground"
        : "bg-accent text-muted-foreground",
      className
    )}
  >
    {children}
  </span>
);

/** The submit chord as this keyboard has it: Command and Enter, or Ctrl and Enter. */
export const SubmitChordHint: React.FC<Pick<Props, "tone" | "className">> = (props) => (
  <KeyHint data-testid='submit-chord-hint' {...props}>
    {isApplePlatform() ? <Command className='size-3' /> : "Ctrl"}
    <CornerDownLeft className='size-3' />
  </KeyHint>
);
