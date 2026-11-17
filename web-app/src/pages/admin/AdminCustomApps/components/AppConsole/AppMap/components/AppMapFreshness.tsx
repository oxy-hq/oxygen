import { RotateCw, TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { errMessage } from "@/hooks/api/errMessage";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { MapFreshness } from "../appMapModel";
import { freshnessText } from "../describe";

/**
 * Where the right-hand side came from, said above the map — the three outcomes kept
 * apart, as the console's rule asks: still reading, could not read (the right side
 * is then the manifest's alone, and says so), and read, with the window it covers.
 */
export const AppMapFreshness = ({
  freshness,
  isError,
  error,
  onRetry,
  untracedSecrets
}: {
  freshness: MapFreshness | null;
  isError: boolean;
  error: unknown;
  onRetry: () => void;
  untracedSecrets: string[];
}) => {
  if (isError) {
    return (
      <div
        className={cn("flex flex-wrap items-center gap-x-2 gap-y-1 text-xs", ADMIN_TONE.warn.text)}
        data-testid='admin-app-map-traces-error'
      >
        <TriangleAlert className='size-3 shrink-0' />
        <span>
          Couldn&rsquo;t read recent runs ({errMessage(error, "no detail")}). Only what the manifest
          declares is drawn on the right.
        </span>
        <Button variant='ghost' size='sm' className='h-6 gap-1 px-1.5 text-xs' onClick={onRetry}>
          <RotateCw className='size-3' />
          Retry
        </Button>
      </div>
    );
  }
  if (!freshness) {
    return (
      <p className='text-muted-foreground text-xs' data-testid='admin-app-map-traces-pending'>
        Reading each function&rsquo;s latest recorded runs&hellip;
      </p>
    );
  }
  return (
    <div className='flex flex-col gap-0.5 text-muted-foreground text-xs'>
      <p data-testid='admin-app-map-freshness'>{freshnessText(freshness, timeAgo).join(" ")}</p>
      {untracedSecrets.length > 0 ? (
        <p data-testid='admin-app-map-untraced-secrets'>
          Declared secrets no recorded call names (read through <code>ctx.env</code>, which is not
          traced per function):{" "}
          <span className='font-mono text-foreground'>{untracedSecrets.join(", ")}</span>
        </p>
      ) : null}
    </div>
  );
};
