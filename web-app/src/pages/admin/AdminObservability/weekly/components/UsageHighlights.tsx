import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE, type AdminTone } from "@/pages/admin/components/adminTone";
import type { UsageHighlight } from "@/types/usageReport";
import { appConsolePath, highlightLabel, highlightsByTone } from "../utils";

/**
 * What changed, in three groups: what needs a look, what is going well, and what was
 * published and never opened.
 *
 * The tone is stated once, on the group, not once per row — every row in a group shares
 * it, and a column of identical dots is decoration.
 */
export function UsageHighlights({ highlights }: { highlights: readonly UsageHighlight[] }) {
  const { attention, good, idle } = highlightsByTone(highlights);

  return (
    <>
      {/* Always present: "nothing needs a look" is an answer the reader came for, and an
          absent section cannot tell it apart from a report that failed to say. The dot
          goes quiet with it — colour here means something is wrong. */}
      <HighlightGroup
        id='attention'
        title='Needs a look'
        tone={attention.length > 0 ? "warn" : "muted"}
      >
        {attention.length > 0 ? (
          <HighlightList highlights={attention} />
        ) : (
          <p
            className='text-muted-foreground text-xs'
            data-testid='admin-usage-report-highlights-attention-empty'
          >
            Nothing needs a look this week.
          </p>
        )}
      </HighlightGroup>

      {good.length > 0 ? (
        <HighlightGroup id='good' title='Going well' tone='ok'>
          <HighlightList highlights={good} />
        </HighlightGroup>
      ) : null}

      {idle.length > 0 ? (
        <HighlightGroup id='idle' title='Published but not opened' tone='muted'>
          <ul className='flex flex-wrap gap-x-5 gap-y-1.5'>
            {idle.map((h) => (
              <li
                key={`${h.app_id}-${h.kind}`}
                className='flex items-baseline gap-1.5 text-xs'
                data-testid={`admin-usage-report-highlight-${h.app_id}-${h.kind}`}
              >
                <Link to={appConsolePath(h.org_slug, h.app_slug)} className='hover:underline'>
                  {h.app_name}
                </Link>
                {/* The same app is often published to several organizations, so a bare
                    list of names would read "Store Ops, Store Ops, Store Ops". */}
                <span className='text-muted-foreground'>{h.org_name}</span>
              </li>
            ))}
          </ul>
        </HighlightGroup>
      ) : null}
    </>
  );
}

function HighlightGroup({
  id,
  title,
  tone,
  children
}: {
  id: "attention" | "good" | "idle";
  title: string;
  tone: AdminTone;
  children: ReactNode;
}) {
  return (
    <section className='space-y-2' data-testid={`admin-usage-report-highlights-${id}`}>
      <h3 className='flex items-center gap-2 font-semibold text-sm'>
        <span className={cn("size-2 shrink-0 rounded-full", ADMIN_TONE[tone].dot)} aria-hidden />
        {title}
      </h3>
      {children}
    </section>
  );
}

function HighlightList({ highlights }: { highlights: readonly UsageHighlight[] }) {
  return (
    <ul className='divide-y divide-border/60 rounded-lg border border-border/60'>
      {highlights.map((h) => (
        <li
          key={`${h.app_id}-${h.kind}`}
          className='flex gap-3 px-3 py-2.5'
          data-testid={`admin-usage-report-highlight-${h.app_id}-${h.kind}`}
        >
          {/* A fixed width from the scale, so the labels form a column to read down. */}
          <span className='w-28 shrink-0 font-medium text-xs'>{highlightLabel(h.kind)}</span>
          <div className='min-w-0 flex-1 space-y-0.5'>
            <p className='flex flex-wrap items-baseline gap-x-2 text-xs'>
              <Link
                to={appConsolePath(h.org_slug, h.app_slug)}
                className='font-medium hover:underline'
              >
                {h.app_name}
              </Link>
              <span className='text-muted-foreground'>{h.org_name}</span>
            </p>
            {/* The server's sentence, as written, on a line of its own that wraps. */}
            <p
              className='whitespace-pre-wrap break-words text-xs'
              data-testid={`admin-usage-report-highlight-${h.app_id}-${h.kind}-detail`}
            >
              {h.detail}
            </p>
          </div>
        </li>
      ))}
    </ul>
  );
}
