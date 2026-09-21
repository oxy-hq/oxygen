import {
  ExternalLink,
  Eye,
  EyeOff,
  Monitor,
  PanelRight,
  RotateCw,
  Smartphone,
  Tablet
} from "lucide-react";
import { useState } from "react";
import { AppMark } from "@/components/apps/AppMark";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import { Select, SelectContent, SelectItem, SelectTrigger } from "@/components/ui/shadcn/select";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { cn } from "@/libs/shadcn/utils";
import type { CustomApp } from "@/types/apps";
import { resolveBundleUrl } from "../../../../resolveBundleUrl";
import { ActAsOrgButton } from "./ActAsOrgButton";
import { OrgHomeButton } from "./OrgHomeButton";

export type DetailTab = "preview" | "info" | "activity" | "settings";
export type Device = "mobile" | "tablet" | "desktop";
export type ChannelView = "published" | "draft";

export interface DetailToolbarProps {
  app: CustomApp;
  tab: DetailTab;
  device: Device;
  channel: ChannelView;
  channelBusy: boolean;
  onTabChange: (tab: DetailTab) => void;
  onDeviceChange: (device: Device) => void;
  onChannelChange: (channel: ChannelView) => void;
  onReload: () => void;
  /** Whether the dossier (status/manifest/builds/…) is currently shown — a side
   *  column on wide viewports, an overlay drawer on narrow ones. */
  dossierOpen?: boolean;
  /** Toggle the dossier. AppDetail decides column-vs-drawer by width. */
  onToggleDossier?: () => void;
  /**
   * Render the section tab strip. The single-surface AppDetail sets this
   * false — there are no sub-tabs any more, so the bar is just identity +
   * preview controls.
   */
  showTabs?: boolean;
}

const DEVICE_LABELS: Record<Device, { icon: typeof Smartphone; label: string; size: string }> = {
  mobile: { icon: Smartphone, label: "Mobile", size: "390 × 844" },
  tablet: { icon: Tablet, label: "Tablet", size: "820 × 1180" },
  desktop: { icon: Monitor, label: "Desktop", size: "Fill" }
};

const CHANNEL_COPY: Record<ChannelView, { title: string; description: string; confirm: string }> = {
  draft: {
    title: "Switch to draft preview?",
    description: "Shows the latest CI build. Customers still see the published bundle.",
    confirm: "Show draft"
  },
  published: {
    title: "Switch back to published?",
    description: "Shows the bundle the customer currently sees.",
    confirm: "Show published"
  }
};

/**
 * The whole "what app am I looking at + where in it am I + what controls
 * are relevant" surface, collapsed into a single 40px row.
 *
 * Reads left → right as three nested concerns:
 *
 *   [identity]  ·  [section nav]  ·  [contextual controls]
 *
 * Section nav (tabs) is the only stable middle element. Contextual
 * controls reflow per tab: device + reload are Preview-only; channel
 * toggle stays put because the underlying cookie is session-wide; Open
 * stays put because the URL is canonical no matter which section is
 * active.
 *
 * The URL isn't rendered inline — it'd dominate the row at any legible
 * size. Hover Open to read it; click to launch in a new tab.
 */
export const DetailToolbar = ({
  app,
  tab,
  device,
  channel,
  channelBusy,
  onTabChange,
  onDeviceChange,
  onChannelChange,
  onReload,
  dossierOpen,
  onToggleDossier,
  showTabs = true
}: DetailToolbarProps) => {
  const isPreview = tab === "preview";
  // Holds the channel the user is asking to switch to. AlertDialog is
  // open iff this is non-null; null = no pending switch.
  const [pendingChannel, setPendingChannel] = useState<ChannelView | null>(null);

  const ActiveDeviceIcon = DEVICE_LABELS[device].icon;

  const handleChannelClick = (next: ChannelView) => {
    if (next === channel || channelBusy) return;
    setPendingChannel(next);
  };

  const confirmChannelSwitch = () => {
    if (!pendingChannel) return;
    onChannelChange(pendingChannel);
    setPendingChannel(null);
  };

  return (
    <header className='flex min-h-10 shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b bg-background px-3 py-1'>
      {/* Identity — name and where it lives, on one truncating line. The source
          badge that also sat here is in the Identity block of the details panel,
          which is where an operator looks it up. */}
      <div className='flex min-w-0 flex-1 items-center gap-2'>
        <AppMark iconUrl={app.icon_url} name={app.name} size='sm' />
        <span className='truncate font-medium text-xs leading-none'>{app.name}</span>
        <span className='hidden min-w-0 truncate text-muted-foreground text-xs sm:inline'>
          {app.org_slug}/{app.slug}
        </span>
      </div>

      {/* Section nav — only when the host renders sub-tabs. The
          single-surface AppDetail hides this entirely. */}
      {showTabs && (
        <nav className='flex shrink-0 items-center gap-0.5' aria-label='App detail sections'>
          <TabPill active={tab === "preview"} onClick={() => onTabChange("preview")}>
            Preview
          </TabPill>
          <TabPill active={tab === "info"} onClick={() => onTabChange("info")}>
            Info
          </TabPill>
          <TabPill active={tab === "activity"} onClick={() => onTabChange("activity")}>
            Activity
          </TabPill>
          <TabPill active={tab === "settings"} onClick={() => onTabChange("settings")}>
            Settings
          </TabPill>
        </nav>
      )}

      {/* Contextual controls. Preview-only items unmount on other tabs
          so the bar visibly compacts when there's nothing to do. */}
      <div className='flex shrink-0 items-center gap-1'>
        {isPreview && (
          <>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant='ghost'
                  size='icon'
                  className='size-6'
                  onClick={onReload}
                  aria-label='Reload preview'
                >
                  <RotateCw className='size-3.5' />
                </Button>
              </TooltipTrigger>
              <TooltipContent>Reload</TooltipContent>
            </Tooltip>

            {/* Device dropdown. Was three inline pills; collapsed to a
                single trigger so the toolbar stays calm and we have
                room to show the actual viewport dimensions next to
                each option (which the pill icons couldn't carry). */}
            <Select value={device} onValueChange={(v) => onDeviceChange(v as Device)}>
              {/* Trigger renders manually rather than via `<SelectValue>`
                  because Radix's SelectValue mirrors the selected
                  SelectItem's children — including its icon — which
                  doubled up next to the icon we want in the trigger.
                  Rendering off the controlled `device` value gives us
                  one icon + just the short label here, and lets each
                  SelectItem carry the richer label + viewport size
                  inside the dropdown. */}
              <SelectTrigger
                className='h-7 w-auto gap-1.5 border-border bg-transparent px-2 text-xs'
                aria-label='Preview viewport'
              >
                <ActiveDeviceIcon className='size-3.5' />
                <span>{DEVICE_LABELS[device].label}</span>
              </SelectTrigger>
              <SelectContent align='end'>
                {(["mobile", "tablet", "desktop"] as Device[]).map((d) => {
                  const { icon: Icon, label, size } = DEVICE_LABELS[d];
                  return (
                    <SelectItem key={d} value={d}>
                      <span className='flex items-center gap-2'>
                        <Icon className='size-3.5 text-muted-foreground' />
                        <span className='font-medium'>{label}</span>
                        <span className='font-mono text-muted-foreground/70 text-xs'>{size}</span>
                      </span>
                    </SelectItem>
                  );
                })}
              </SelectContent>
            </Select>
          </>
        )}

        {/* Channel pills — visible on every tab because the underlying
            cookie is session-wide. Clicking triggers a confirmation
            dialog rather than flipping immediately, so a stray click
            doesn't accidentally expose draft content. The active
            segment takes the channel's tone (ok / warn) for peripheral
            recognition — spelled out as the `status-*` tokens `ADMIN_TONE`
            is built from, because a `data-[state=on]:` variant has to be a
            literal class for Tailwind to emit it; it cannot be composed
            from the constant at runtime.

            Published segment is disabled when the app has never been
            published — the cookie would do nothing, the iframe would
            still 403 for non-app-admins. */}
        <ToggleGroup
          type='single'
          value={channel}
          onValueChange={(v) => v && handleChannelClick(v as ChannelView)}
          size='sm'
          variant='outline'
          disabled={channelBusy}
          aria-label='Bundle channel'
        >
          <Tooltip>
            <TooltipTrigger asChild>
              <ToggleGroupItem
                value='published'
                aria-label='Show published bundle'
                disabled={!app.published_at}
                className='h-7 gap-1.5 px-2 data-[state=on]:bg-status-success-bg data-[state=on]:text-status-success-text'
              >
                <Eye className='size-3.5' />
                <span className='text-xs'>Published</span>
              </ToggleGroupItem>
            </TooltipTrigger>
            {!app.published_at && (
              <TooltipContent>Publish the app to enable this view.</TooltipContent>
            )}
          </Tooltip>
          <ToggleGroupItem
            value='draft'
            aria-label='Preview draft bundle'
            className='h-7 gap-1.5 px-2 data-[state=on]:bg-status-warning-bg data-[state=on]:text-status-warning-text'
          >
            <EyeOff className='size-3.5' />
            <span className='text-xs'>Draft</span>
          </ToggleGroupItem>
        </ToggleGroup>

        {onToggleDossier && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant='ghost'
                size='icon'
                className={cn("size-7", dossierOpen && "bg-muted text-foreground")}
                onClick={onToggleDossier}
                aria-label={dossierOpen ? "Hide details" : "Show details"}
                aria-pressed={dossierOpen}
              >
                <PanelRight className='size-3.5' />
              </Button>
            </TooltipTrigger>
            <TooltipContent>{dossierOpen ? "Hide details" : "Show details"}</TooltipContent>
          </Tooltip>
        )}

        {/* Sits beside Open because they answer the same question — "let me look
            at the real thing" — and differ only in whose data comes back.
            Org home is the cheap version of the same question and is deliberately
            first: checking the launcher usually answers it without an audited
            session that closes admin for an hour. */}
        <div className='flex items-center gap-1'>
          <OrgHomeButton app={app} />
          <ActAsOrgButton app={app} />
        </div>

        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant='ghost'
              size='icon'
              className='size-6'
              onClick={() =>
                window.open(
                  app.url_subdomain ?? resolveBundleUrl(app.url),
                  "_blank",
                  "noopener,noreferrer"
                )
              }
              aria-label='Open customer URL in a new tab'
            >
              <ExternalLink className='size-3.5' />
            </Button>
          </TooltipTrigger>
          <TooltipContent className='max-w-md space-y-1.5'>
            {app.url_subdomain && (
              <div>
                <span className='block text-xs opacity-60'>Subdomain URL (recommended)</span>
                <span className='block break-all font-mono text-xs'>{app.url_subdomain}</span>
              </div>
            )}
            <div>
              <span className='block text-xs opacity-60'>Subpath URL</span>
              <span className='block break-all font-mono text-xs'>{app.url}</span>
            </div>
          </TooltipContent>
        </Tooltip>
      </div>

      <AlertDialog
        open={pendingChannel !== null}
        onOpenChange={(open) => {
          if (!open && !channelBusy) setPendingChannel(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {pendingChannel ? CHANNEL_COPY[pendingChannel].title : ""}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {pendingChannel ? CHANNEL_COPY[pendingChannel].description : ""}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={channelBusy}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={channelBusy}
              onClick={(e) => {
                e.preventDefault();
                confirmChannelSwitch();
              }}
            >
              {channelBusy
                ? "Switching…"
                : pendingChannel
                  ? CHANNEL_COPY[pendingChannel].confirm
                  : ""}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </header>
  );
};

/**
 * Tab pill. Distinct from a shadcn `TabsTrigger` because we drive
 * routing from the parent (URL-backed) rather than uncontrolled tab
 * state, so we don't need the Radix machinery — and a plain button
 * styled to match is lighter on a row this tight.
 */
const TabPill = ({
  active,
  onClick,
  children
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) => (
  <button
    type='button'
    onClick={onClick}
    aria-pressed={active}
    className={cn(
      "relative h-7 rounded-md px-2.5 font-medium text-xs transition-colors",
      active ? "text-foreground" : "text-muted-foreground hover:bg-muted/60 hover:text-foreground"
    )}
  >
    {children}
    {/* Underline indicator sits flush with the bottom of the toolbar
        (not floating below it) — keeps the bar's bottom edge clean. */}
    {active && (
      <span className='absolute right-2.5 bottom-[-5px] left-2.5 h-0.5 rounded-full bg-foreground' />
    )}
  </button>
);
