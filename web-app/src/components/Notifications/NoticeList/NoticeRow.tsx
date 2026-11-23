import { ArrowSquareOut, Clock, WarningCircle } from "@phosphor-icons/react";
import { useLayoutEffect, useRef, useState } from "react";
import { AppMark } from "@/components/apps/AppMark";
import { OxyMark } from "@/components/OxyMark";
import {
  DESIGN_REFERENCE,
  focusRing,
  iconInline,
  textMd,
  textSm
} from "@/components/ui/reference/tokens";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { cn } from "@/libs/shadcn/utils";
import type { InboxItem } from "@/types/notifications";
import { FollowLink } from "../FollowLink";
import { LEVEL_ACCENT } from "../utils";
import { shortAgo } from "./shortAgo";

/** The sender: the app's 40px mark, or Oxygen's own on a muted square. */
function SenderMark({ item, iconUrl }: { item: InboxItem; iconUrl?: string }) {
  if (item.app) {
    return (
      <AppMark iconUrl={iconUrl} name={item.app.name} size='xl' className='size-10 border-0' />
    );
  }
  return (
    <span aria-hidden className='flex size-10 shrink-0 items-center justify-center bg-muted'>
      <OxyMark className='size-5 text-foreground' />
    </span>
  );
}

/** A critical or time-sensitive kind says so before its title; the rest carry nothing. */
function LevelMark({ item }: { item: InboxItem }) {
  const accent = item.level ? LEVEL_ACCENT[item.level] : undefined;
  if (!accent) return null;
  const Icon = item.level === "critical" ? WarningCircle : Clock;
  const tone = item.level === "critical" ? "text-status-error" : "text-status-warning";
  return (
    <>
      <Icon aria-hidden className={cn(iconInline, "mr-1 inline align-[-2px]", tone)} />
      <span className='sr-only'>{accent.label}: </span>
    </>
  );
}

/** Whether a line-clamped text is cut short; measured while folded, kept while open. */
function useClamped(open: boolean) {
  const ref = useRef<HTMLSpanElement>(null);
  const [clamped, setClamped] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || open) return;
    const measure = () => setClamped(el.scrollHeight > el.clientHeight + 1);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [open]);
  return { ref, clamped };
}

/** The detail, two lines; cut short, pressing it unfolds it (and reads nothing). */
function Detail({ body }: { body: string }) {
  const [open, setOpen] = useState(false);
  const { ref, clamped } = useClamped(open);
  return (
    // The detail runs the row's width, short of the open icon in its corner.
    <span className='flex min-w-0 pr-8'>
      {/* Always this one button, so the measured text never remounts. Cut short it sits
          above the row's own button; whole, it lets presses through to the row. */}
      <button
        type='button'
        disabled={!clamped}
        aria-expanded={clamped ? open : undefined}
        aria-label={clamped ? (open ? "Show less" : "Show the whole text") : undefined}
        onClick={() => setOpen((o) => !o)}
        className={cn(
          "min-w-0 text-left",
          clamped ? "relative z-10 cursor-pointer" : "pointer-events-none",
          focusRing
        )}
      >
        <span ref={ref} className={cn("block text-muted-foreground", !open && "line-clamp-2")}>
          {body}
        </span>
      </button>
    </span>
  );
}

/**
 * One notification, a row of the stacked list: the sender's mark; the sender
 * with when (and the unread dot) on the first line; the title; the detail in a
 * quieter ink; and the way to what it is about as an icon in the row's corner.
 * Pressing the row reads it; the icon follows its link and reads it.
 *
 * Transcribed from oxy-hq/reference-app `src/views/parts/HomeNotices.tsx`
 * (`NoticeRow`, branch design/home-match-oxygen) onto the inbox's rows.
 */
export function NoticeRow({
  item,
  iconUrl,
  onRead,
  onFollow
}: {
  item: InboxItem;
  /** The sending app's icon, when the org's app list has it. */
  iconUrl?: string;
  onRead: () => void;
  /** After the row's link is followed: a sheet closes itself. */
  onFollow?: () => void;
}) {
  const unread = item.read_at === null;
  const name = item.app?.name ?? "Oxygen";
  const readIfUnread = () => {
    if (unread) onRead();
  };
  return (
    <li
      className={cn(
        "relative transition-colors has-[>button:hover]:bg-row-hover",
        // New on the card; read, it steps back onto the panel's own ground.
        unread ? "bg-card" : "bg-background"
      )}
      data-testid={`notifications-item-${item.id}`}
      data-unread={unread}
      data-level={item.level}
    >
      <div className='flex w-full items-start gap-3 px-4 py-3'>
        <SenderMark item={item} iconUrl={iconUrl} />
        {unread && <span className='sr-only'>Unread. </span>}
        <span className={cn(textMd, "flex min-w-0 flex-1 flex-col")}>
          <span className='flex min-w-0 items-baseline justify-between gap-3'>
            <span className='truncate font-semibold text-foreground'>{name}</span>
            <span
              className={cn(
                textSm,
                "flex shrink-0 items-center gap-2 text-disabled-foreground tabular-nums"
              )}
            >
              <time dateTime={item.created_at}>{shortAgo(item.created_at)}</time>
              {unread && (
                <span aria-hidden className='size-[7px] shrink-0 rounded-full bg-dot-error' />
              )}
            </span>
          </span>
          {item.title === null ? (
            <span className='text-muted-foreground italic'>Couldn't load this notification</span>
          ) : (
            <span
              className={cn(
                "truncate font-semibold",
                unread ? "text-foreground" : "text-dim-foreground"
              )}
            >
              <LevelMark item={item} />
              {item.title}
            </span>
          )}
          {item.body && <Detail body={item.body} />}
        </span>
      </div>
      {/* The row itself, pressed, reads it: a clear button laid over it, under its controls. */}
      <button
        type='button'
        onClick={readIfUnread}
        aria-label={unread ? `Mark read: ${item.title ?? name}` : (item.title ?? name)}
        className={cn("absolute inset-0", focusRing)}
      />
      {/* Where it points, in the row's bottom-right corner, out of the text's lines. Drawn
          28px; its hit area grows to 44px around it. The name opens leftward in a hint. */}
      {item.href && (
        <Tooltip delayDuration={300}>
          <TooltipTrigger asChild>
            <FollowLink
              href={item.href}
              onClick={() => {
                readIfUnread();
                onFollow?.();
              }}
              aria-label={`Open in ${name}`}
              data-testid={`notifications-item-open-${item.id}`}
              className={cn(
                "absolute right-2.5 bottom-2 z-10 flex size-7 items-center justify-center text-muted-foreground hover:bg-row-hover hover:text-foreground",
                "before:absolute before:-inset-2 before:content-['']",
                focusRing
              )}
            >
              <ArrowSquareOut aria-hidden className={iconInline} />
            </FollowLink>
          </TooltipTrigger>
          <TooltipContent side='left' sideOffset={4} className={DESIGN_REFERENCE}>
            Open in {name}
          </TooltipContent>
        </Tooltip>
      )}
    </li>
  );
}
