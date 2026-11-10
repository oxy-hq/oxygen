// The wired shell's clock: a button that opens Today, the person's widget
// cards from every app they can open, so an item that needs them is one click
// away from inside any custom app (the web-app's top bar does the same).
//
//   GET /api/home/widgets?org=<slug>   (the SDK's credentialed fetcher)
//
// Lazy: nothing is requested until the panel is first opened, and a reopen
// asks again only when the last answer is a minute old or failed. Never throws
// into the host app: a rejected request, a non-2xx or a payload that is not a
// widget list all end as "Couldn't load Today." (see todayModel.ts).
//
// In the phone layout an open panel holds a history entry, as a PhoneSheet does,
// so the phone's Back closes it and leaves the app where it was. Elsewhere (a
// desktop, a tablet) it holds none: Back there is the browser's, as it was.

import { type MouseEvent, useCallback, useEffect, useId, useRef, useState } from "react";
import { useOxyApp } from "../custom-app/react";
import { cx } from "./cx";
import { followRow } from "./NavSheet";
import { holdHistoryEntry } from "./phone/sheetHistory";
import { usePhoneLayout } from "./phone/usePhoneLayout";
import { useClockText } from "./TopBar";
import { type TodayRow, todayRows } from "./todayModel";

/** A reopen within this window shows what it already has. */
const FRESH_MS = 60_000;
/** Backstop for the exit animation's `animationend`, which a hidden tab or a
 *  stylesheet-less host never fires. Longer than the slowest exit. */
const EXIT_MS = 240;

type Phase = "closed" | "open" | "closing";
type Status = "idle" | "loading" | "ready" | "error";

function ClockIcon() {
  return (
    <svg
      className='oxy-clock__icon'
      width='16'
      height='16'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
    >
      <circle cx='12' cy='12' r='10' />
      <path d='M12 6v6l4 2' />
    </svg>
  );
}

/** The count ("85/97"), not a ring: at this size an empty ring reads as a radio button. */
function Count({ done, total }: { done: number; total: number }) {
  return (
    <span className='oxy-today__count'>
      {done}/{total}
    </span>
  );
}

function RowMark({ row, iconUrl }: { row: TodayRow; iconUrl: string | undefined }) {
  const [broken, setBroken] = useState(false);
  return (
    <span className='oxy-today__mark' aria-hidden='true'>
      {iconUrl && !broken ? (
        <img src={iconUrl} alt='' className='oxy-today__mark-img' onError={() => setBroken(true)} />
      ) : (
        row.initials
      )}
    </span>
  );
}

function Row({
  row,
  resolve,
  onPick
}: {
  row: TodayRow;
  resolve: (url?: string | null) => string | undefined;
  onPick: (e: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const href = row.href ? (resolve(row.href) ?? row.href) : undefined;
  const iconUrl = row.iconUrl ? (resolve(row.iconUrl) ?? row.iconUrl) : undefined;
  const body = (
    <>
      <RowMark row={row} iconUrl={iconUrl} />
      <span className='oxy-today__body'>
        <span className='oxy-today__row-title'>{row.title}</span>
        <span className='oxy-today__row-headline'>{row.headline}</span>
        {row.detail && <span className='oxy-today__row-detail'>{row.detail}</span>}
      </span>
      {/* Counts only where someone has a job: "0/17" beside "All checklists on time" is noise. */}
      {row.progress && row.group === "attention" && (
        <Count done={row.progress.done} total={row.progress.total} />
      )}
    </>
  );
  const className = cx("oxy-today__row", `oxy-today__row--${row.group}`);
  return (
    <li className='oxy-today__item'>
      {href ? (
        <a
          href={href}
          className={className}
          data-testid='topbar-today-row'
          data-group={row.group}
          onClick={onPick}
        >
          {body}
        </a>
      ) : (
        <div className={className} data-testid='topbar-today-row' data-group={row.group}>
          {body}
        </div>
      )}
    </li>
  );
}

function Message({ children, testId }: { children: string; testId: string }) {
  return (
    <p className='oxy-today__message' data-testid={testId} role='status'>
      {children}
    </p>
  );
}

/**
 * The top bar clock as a button that opens Today. `orgSlug` names whose
 * widgets to read; `homeHref` is the product home (`links.home`) the footer
 * opens, and `resolve` makes a product path absolute when the host set
 * `productBaseUrl`.
 */
export function TodayClock({
  orgSlug,
  homeHref,
  resolve,
  timezone
}: {
  orgSlug: string;
  homeHref: string;
  resolve: (url?: string | null) => string | undefined;
  timezone?: string;
}) {
  const { fetcher } = useOxyApp();
  const clock = useClockText(timezone);
  const phone = usePhoneLayout();
  const panelId = useId();
  const [phase, setPhase] = useState<Phase>("closed");
  const [status, setStatus] = useState<Status>("idle");
  const [rows, setRows] = useState<TodayRow[] | null>(null);
  const wrapRef = useRef<HTMLSpanElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const request = useRef(0);
  const loadedAt = useRef<number | null>(null);
  const hasRows = useRef(false);

  // A newer org or fetcher invalidates what an older request is about to say.
  useEffect(() => {
    return () => {
      request.current += 1;
    };
  }, []);

  const load = useCallback(() => {
    const id = ++request.current;
    if (!hasRows.current) setStatus("loading");
    Promise.resolve()
      .then(() =>
        fetcher(`/api/home/widgets?org=${encodeURIComponent(orgSlug)}`, { method: "GET" })
      )
      .then(async (resp) => {
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        return todayRows(await resp.json());
      })
      .then((next) => {
        if (id !== request.current) return;
        if (!next) throw new Error("not a widget list");
        hasRows.current = true;
        loadedAt.current = Date.now();
        setRows(next);
        setStatus("ready");
      })
      .catch(() => {
        // A failed refresh keeps the last good rows; a first load says so.
        if (id === request.current && !hasRows.current) setStatus("error");
      });
  }, [fetcher, orgSlug]);

  const show = () => {
    setPhase("open");
    const stale = loadedAt.current === null || Date.now() - loadedAt.current > FRESH_MS;
    if (stale) load();
  };

  const hide = useCallback((returnFocus: boolean) => {
    setPhase((p) => (p === "open" ? "closing" : p));
    if (returnFocus) buttonRef.current?.focus();
  }, []);

  // Closing is a state of its own so the exit can play; it ends on the
  // animation, or on a timer where no animation runs.
  useEffect(() => {
    if (phase !== "closing") return;
    const t = setTimeout(() => setPhase("closed"), EXIT_MS);
    return () => clearTimeout(t);
  }, [phase]);

  // Open: focus moves to the panel (a keyboard user lands in it), Escape
  // closes and returns focus to the clock, and a press outside closes.
  useEffect(() => {
    if (phase !== "open") return;
    panelRef.current?.focus({ preventScroll: true });
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") hide(true);
    };
    // A press outside, or focus tabbing past the panel's end, leaves it.
    const onAway = (e: Event) => {
      if (!wrapRef.current?.contains(e.target as Node)) hide(false);
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onAway);
    document.addEventListener("focusin", onAway);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onAway);
      document.removeEventListener("focusin", onAway);
    };
  }, [phase, hide]);

  // The phone's Back closes an open panel and leaves the page where it was.
  const holding = phone && phase === "open";
  useEffect(() => {
    if (holding) return holdHistoryEntry(() => hide(false));
  }, [holding, hide]);

  // With an entry held, a link's own navigation would leave it between this page
  // and the next, and the close's step back would race the load: the link replaces
  // the entry instead, as a drawer row does. Without one, a link is a link.
  const pick = (e: MouseEvent<HTMLAnchorElement>) => {
    if (holding) followRow(e);
    hide(false);
  };

  const home = resolve(homeHref) ?? homeHref;
  const present = phase !== "closed";
  return (
    <span ref={wrapRef} className='oxy-shell-scope oxy-today-anchor'>
      <button
        ref={buttonRef}
        type='button'
        data-testid='topbar-clock'
        className='oxy-clock oxy-clock--button'
        aria-haspopup='dialog'
        aria-expanded={phase === "open"}
        aria-controls={present ? panelId : undefined}
        aria-label={`Today, ${clock}`}
        onClick={() => (phase === "open" ? hide(false) : show())}
      >
        <ClockIcon />
        <span className='oxy-clock__text'>{clock}</span>
      </button>
      {present && (
        <div
          ref={panelRef}
          id={panelId}
          role='dialog'
          aria-label='Today'
          tabIndex={-1}
          data-testid='topbar-today'
          data-state={phase === "open" ? "open" : "closed"}
          className='oxy-today'
          onAnimationEnd={(e) => {
            if (e.target === e.currentTarget) setPhase((p) => (p === "closing" ? "closed" : p));
          }}
        >
          <h2 className='oxy-today__title'>Today</h2>
          <div className='oxy-today__scroll'>
            {rows?.length ? (
              <ul className='oxy-today__list'>
                {rows.map((row) => (
                  <Row key={row.key} row={row} resolve={resolve} onPick={pick} />
                ))}
              </ul>
            ) : rows ? (
              <Message testId='topbar-today-empty'>Nothing for you today.</Message>
            ) : status === "error" ? (
              <Message testId='topbar-today-error'>Couldn't load Today.</Message>
            ) : (
              <Message testId='topbar-today-loading'>Loading…</Message>
            )}
          </div>
          <a
            href={home}
            className='oxy-today__footer'
            data-testid='topbar-today-home'
            onClick={holding ? pick : undefined}
          >
            Open Oxygen home
          </a>
        </div>
      )}
    </span>
  );
}
