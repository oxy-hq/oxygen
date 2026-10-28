import { LayoutGrid } from "lucide-react";
import { possessive } from "@/libs/utils/possessive";

/**
 * The home's apps section before any app is published. Apps are built by the
 * customer's Oxy team, so this says they are coming rather than offering a setup
 * step the customer does not own.
 */
export function FirstAppsOnTheirWay({ orgName }: { orgName?: string }) {
  return (
    <div
      className='flex items-start gap-4 rounded-lg border border-dashed bg-card p-6'
      data-testid='home-first-apps'
    >
      <div className='flex size-10 shrink-0 items-center justify-center rounded-md bg-muted'>
        <LayoutGrid className='size-5 text-muted-foreground' aria-hidden />
      </div>
      <div className='flex max-w-prose flex-col gap-1'>
        <p className='font-medium text-base'>Your first apps are on their way</p>
        <p className='text-muted-foreground text-sm'>
          Your Oxy team is building {orgName ? possessive(orgName) : "your"} apps. They'll show up
          here as soon as they're published.
        </p>
      </div>
    </div>
  );
}
