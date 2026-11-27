import { ArrowUpCircle, History, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import type { AppBuild } from "@/types/apps";
import { shortBuildId } from "./lifecycleCopy";

/** The two of the card's verbs that the promote-checks gate shapes. */

/**
 * "Promote to production". While a check the app requires is still open it waits:
 * disabled, spinning, and saying so. It steps back to an outline when going back is
 * the first thing to do.
 */
export const PromoteButton = ({
  waits = false,
  secondary,
  disabled,
  onClick
}: {
  /** A check the app requires is still open on the staging build. */
  waits?: boolean;
  /** Production's smoke failed, so a rollback is the primary action. */
  secondary: boolean;
  disabled: boolean;
  onClick: () => void;
}) => (
  <Button
    size='sm'
    variant={secondary ? "outline" : "default"}
    className='h-7 gap-1.5 text-xs'
    disabled={disabled || waits}
    onClick={onClick}
    data-testid='app-lifecycle-promote'
    data-waiting={waits ? true : undefined}
  >
    {waits ? (
      <Loader2 className='size-3.5 animate-spin motion-reduce:animate-none' />
    ) : (
      <ArrowUpCircle className='size-3.5' />
    )}
    {waits ? "Promote · waiting on checks" : "Promote to production"}
  </Button>
);

/** "Roll back to 0123456": straight to the build production served before its smoke
 *  failed, without the menu. */
export const RollbackBeforeButton = ({
  build,
  disabled,
  onPick
}: {
  build: AppBuild;
  disabled: boolean;
  onPick: (b: AppBuild) => void;
}) => (
  <Button
    size='sm'
    className='h-7 gap-1.5 text-xs'
    disabled={disabled}
    onClick={() => onPick(build)}
    data-testid='app-lifecycle-rollback-before'
  >
    <History className='size-3.5' />
    Roll back to {shortBuildId(build.build_id)}
  </Button>
);
