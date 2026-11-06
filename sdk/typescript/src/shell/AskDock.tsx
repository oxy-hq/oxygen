// Ask Oxygen inside a custom app: the same right-side dock the main
// web-app opens with ⌘K, powered by the SDK's bundle-gated agent-run
// stream (`useAgentRun`). Multi-turn: follow-ups reuse the run's thread.
//
// History is session-scoped: bundles have no server list-threads endpoint,
// so "History" lists chats started in this dock session (restoring one
// resumes its server thread for follow-ups via the SSE resume). Beyond a
// quick ask, the header links to the product Chat surface. No
// extended-thinking toggle — the bundle ask API doesn't carry it.

import * as React from "react";
import { type AgentRunEvent, OxyAnswer, useAgentRun, useOxyApp } from "../custom-app/react";
import { AnswerChart, type ChartBlock } from "./AnswerChart";
import { cx } from "./cx";
import { OxyMark } from "./marks";
import { ReasoningTrace } from "./ReasoningTrace";
import { fetchThreadTranscript, useThreadHistory } from "./threadHistory";
import {
  aggregateLlmStats,
  buildTraceSteps,
  extractAnswer,
  extractClarification,
  type TraceStep
} from "./trace";

function CloseIcon() {
  return (
    <svg
      width='14'
      height='14'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
    >
      <path d='M18 6 6 18' />
      <path d='m6 6 12 12' />
    </svg>
  );
}

function PlusIcon() {
  return (
    <svg
      width='15'
      height='15'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
    >
      <path d='M5 12h14' />
      <path d='M12 5v14' />
    </svg>
  );
}

function HistoryIcon() {
  return (
    <svg
      width='14'
      height='14'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
    >
      <path d='M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8' />
      <path d='M3 3v5h5' />
      <path d='M12 7v5l4 2' />
    </svg>
  );
}

function SearchIcon() {
  return (
    <svg
      width='14'
      height='14'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
    >
      <circle cx='11' cy='11' r='8' />
      <path d='m21 21-4.3-4.3' />
    </svg>
  );
}

function ThreadIcon() {
  return (
    <svg
      width='15'
      height='15'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
    >
      <path d='M14 9a2 2 0 0 1-2 2H6l-4 4V4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2z' />
      <path d='M18 9h2a2 2 0 0 1 2 2v11l-4-4h-6a2 2 0 0 1-2-2v-1' />
    </svg>
  );
}

function ExternalIcon() {
  return (
    <svg
      width='14'
      height='14'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
    >
      <path d='M15 3h6v6' />
      <path d='M10 14 21 3' />
      <path d='M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6' />
    </svg>
  );
}

function ArrowUpIcon() {
  return (
    <svg
      width='14'
      height='14'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
    >
      <path d='m5 12 7-7 7 7' />
      <path d='M12 19V5' />
    </svg>
  );
}

function StopIcon() {
  return (
    <svg width='12' height='12' viewBox='0 0 24 24' fill='currentColor' aria-hidden='true'>
      <rect x='6' y='6' width='12' height='12' rx='2' />
    </svg>
  );
}

export interface AskDockProps {
  /** Agent the panel asks (the app manifest's `ask.agent`). */
  agentId: string;
  /** Workspace display name for the composer placeholder. */
  workspaceName?: string;
  /** Suggested-question chips shown before the first ask. */
  suggestions?: string[];
  /** "Open chat in Oxygen" header link target (product Chat surface). */
  threadsHref?: string;
  open: boolean;
  onClose: () => void;
  className?: string;
}

interface CompletedTurn {
  question: string;
  answer: string | null;
  charts: ChartBlock[];
  steps: TraceStep[];
  llm: { calls: number; totalMs: number };
}

interface Conversation {
  /** Stable key — the server thread id, or the first question when the
   *  thread never got assigned (e.g. a run that failed before starting). */
  id: string;
  title: string;
  turns: CompletedTurn[];
  threadId: string | null;
  /** ISO timestamp the conversation was started — the server thread's
   *  `created_at` when restored, else when its first question was asked.
   *  Drives the history list's time label and its created_at ordering. */
  createdAt: string;
}

function chartsFrom(events: AgentRunEvent[]): ChartBlock[] {
  return events
    .filter((ev) => ev.type === "chart_rendered")
    .map((ev) => ev.data as ChartBlock)
    .filter((b) => b?.config && Array.isArray(b.columns) && Array.isArray(b.rows));
}

/** History label: a relative "N minutes/hours/days ago" for when the
 *  conversation was started. */
function formatHistoryTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const s = Math.max(0, Math.round((Date.now() - d.getTime()) / 1000));
  if (s < 45) return "just now";
  const min = Math.round(s / 60);
  if (min < 60) return `${min} minute${min === 1 ? "" : "s"} ago`;
  const hr = Math.round(s / 3600);
  if (hr < 24) return `${hr} hour${hr === 1 ? "" : "s"} ago`;
  const day = Math.round(s / 86400);
  if (day < 30) return `${day} day${day === 1 ? "" : "s"} ago`;
  const mo = Math.round(day / 30);
  if (mo < 12) return `${mo} month${mo === 1 ? "" : "s"} ago`;
  const yr = Math.round(day / 365);
  return `${yr} year${yr === 1 ? "" : "s"} ago`;
}

/** How many history rows to show before the "Show more" button. */
const HISTORY_PAGE_SIZE = 12;

/** Rebuild a completed turn from a transcript turn's processed events —
 *  identical to what the live dock accumulates during a run. */
function turnFromEvents(question: string, events: AgentRunEvent[]): CompletedTurn {
  // A turn that ended in a clarifying question has no `text_delta` answer —
  // fall back to the suspension prompt so it doesn't restore as a blank bubble.
  return {
    question,
    answer: extractAnswer(events) || extractClarification(events) || null,
    charts: chartsFrom(events),
    steps: buildTraceSteps(events),
    llm: aggregateLlmStats(events)
  };
}

/**
 * The Ask Oxygen drawer. Keep it mounted and toggle `open` — the
 * transcript survives close/reopen, matching the web-app dock.
 */
export function AskDock({
  agentId,
  workspaceName,
  suggestions = [],
  threadsHref,
  open,
  onClose,
  className
}: AskDockProps) {
  const run = useAgentRun({ agentId });
  const { projectId, fetcher } = useOxyApp();
  // Persistent, per-account history (bundle-gated API). Merged with the
  // richer in-session conversations below; empty on older servers.
  const { threads: serverThreads, refetch: refetchHistory } = useThreadHistory();
  const [turns, setTurns] = React.useState<CompletedTurn[]>([]);
  const [pending, setPending] = React.useState<string | null>(null);
  const [draft, setDraft] = React.useState("");
  // Thread of the active conversation. Mirrors run.threadId while a chat is
  // live but is cleared on "new chat" so the next ask starts a fresh thread.
  const [threadId, setThreadId] = React.useState<string | null>(null);
  const [history, setHistory] = React.useState<Conversation[]>([]);
  const [historyOpen, setHistoryOpen] = React.useState(false);
  // History panel search text + how many rows are revealed ("Show more").
  const [historyQuery, setHistoryQuery] = React.useState("");
  const [historyShown, setHistoryShown] = React.useState(HISTORY_PAGE_SIZE);
  const [restoring, setRestoring] = React.useState(false);
  const scrollRef = React.useRef<HTMLDivElement>(null);
  const inputRef = React.useRef<HTMLTextAreaElement>(null);
  // Generation guard for the async transcript restore: any conversation
  // switch bumps this, and an in-flight `openServerThread` fetch that
  // resolves afterwards checks it and bails instead of clobbering the now-
  // current turns (which would leave the visible transcript and the resumed
  // thread pointing at different conversations).
  const restoreGenRef = React.useRef(0);
  // Start time of the active conversation, captured once so the history
  // entry keeps a stable created_at (set on the first ask, or to the server
  // thread's created_at when one is restored; cleared on "new chat").
  const startedAtRef = React.useRef<string | null>(null);

  const busy = run.state === "running";

  // The reasoning trace mirrors the web-app's analytics view: fold the raw
  // SSE events into step rows + tool/SQL items as they stream. Charts come
  // straight from the pipeline's chart_rendered blocks, rendered natively.
  const liveSteps = React.useMemo(() => buildTraceSteps(run.events), [run.events]);
  const liveLlm = React.useMemo(() => aggregateLlmStats(run.events), [run.events]);
  const liveCharts = React.useMemo(() => chartsFrom(run.events), [run.events]);

  // The drawer stays mounted, so opening it (⌘K or the top-bar button) must
  // move focus to the composer itself — otherwise the first keystrokes go
  // nowhere until the user clicks in. Absent while History is showing.
  React.useEffect(() => {
    if (open) inputRef.current?.focus();
  }, [open]);

  // Adopt the server-assigned thread id as it arrives; "new chat" resets the
  // local value to null and the old run.threadId won't re-trigger this.
  React.useEffect(() => {
    if (run.threadId) setThreadId(run.threadId);
  }, [run.threadId]);

  const finalizedPendingTurn = (): CompletedTurn | null =>
    pending === null
      ? null
      : {
          question: pending,
          answer: run.answer ?? run.clarification,
          charts: liveCharts,
          steps: liveSteps,
          llm: liveLlm
        };

  /** The current conversation as a snapshot, or null when it's empty. */
  const snapshotCurrent = (): Conversation | null => {
    const finalized = finalizedPendingTurn();
    const all = finalized ? [...turns, finalized] : turns;
    if (all.length === 0) return null;
    return {
      id: threadId ?? all[0].question,
      title: all[0].question,
      turns: all,
      threadId,
      createdAt: startedAtRef.current ?? new Date().toISOString()
    };
  };

  const resetToEmpty = () => {
    restoreGenRef.current += 1;
    run.cancel();
    setTurns([]);
    setPending(null);
    setDraft("");
    setThreadId(null);
    startedAtRef.current = null;
  };

  const newChat = () => {
    const snap = snapshotCurrent();
    if (snap) setHistory((h) => [snap, ...h.filter((c) => c.id !== snap.id)]);
    resetToEmpty();
    setHistoryOpen(false);
  };

  const openConversation = (conv: Conversation) => {
    restoreGenRef.current += 1;
    const snap = snapshotCurrent();
    setHistory((h) => {
      const rest = h.filter((c) => c.id !== conv.id && (!snap || c.id !== snap.id));
      return snap && snap.id !== conv.id ? [snap, ...rest] : rest;
    });
    run.cancel();
    setTurns(conv.turns);
    setPending(null);
    setDraft("");
    setThreadId(conv.threadId);
    setHistoryOpen(false);
  };

  /** Restore a persistent (server) thread: archive the current chat, then
   *  fetch + rebuild the transcript. Follow-ups resume the same thread. */
  const openServerThread = async (id: string) => {
    if (!projectId) return;
    const gen = (restoreGenRef.current += 1);
    const snap = snapshotCurrent();
    if (snap && snap.id !== id) {
      setHistory((h) => [snap, ...h.filter((c) => c.id !== snap.id)]);
    }
    run.cancel();
    setTurns([]);
    setPending(null);
    setDraft("");
    setThreadId(id);
    // Preserve the restored thread's own created_at for its history label.
    startedAtRef.current =
      serverThreads.find((t) => t.id === id)?.created_at ?? new Date().toISOString();
    setHistoryOpen(false);
    setRestoring(true);
    try {
      const transcript = await fetchThreadTranscript(fetcher, projectId, id);
      // The user switched conversations while this fetch was in flight —
      // drop the stale result rather than overwrite the current turns.
      if (restoreGenRef.current !== gen) return;
      setTurns(transcript.turns.map((t) => turnFromEvents(t.question, t.events)));
    } catch {
      // Non-fatal: leave the transcript empty; the thread still resumes on
      // the next question (the server has its full context).
    } finally {
      if (restoreGenRef.current === gen) setRestoring(false);
    }
  };

  const send = (raw: string) => {
    const question = raw.trim();
    if (!question || busy) return;
    // A new question supersedes any in-flight restore for the prior context.
    restoreGenRef.current += 1;
    // First ask of a fresh chat stamps its start time.
    if (startedAtRef.current === null) startedAtRef.current = new Date().toISOString();
    if (pending !== null) {
      // Archive the finished turn before the hook resets for the next run.
      // A clarification prompt stands in for the answer so the exchange
      // stays readable in the transcript.
      const finalized = finalizedPendingTurn();
      if (finalized) setTurns((t) => [...t, finalized]);
    }
    setPending(question);
    setDraft("");
    run.ask(question, threadId ? { threadId } : undefined);
  };

  // Follow the stream: after every render, pin the scroll to the bottom —
  // unless the user has scrolled up to read something, in which case leave
  // them alone. Deliberately no dependency array: tokens arrive on most
  // renders while streaming, and the near-bottom check makes it a no-op
  // otherwise.
  React.useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
    if (nearBottom) el.scrollTo({ top: el.scrollHeight });
  });

  const empty = pending === null && turns.length === 0;
  const canNewChat = !empty;

  // History panel: the active conversation plus every past one — the active
  // one stays in the list (flagged `active` so the UI highlights it rather
  // than hiding it). Matches snapshotCurrent()'s id so it dedupes cleanly.
  const currentId = threadId ?? turns[0]?.question ?? pending ?? null;
  const historyEntries: Array<{
    id: string;
    title: string;
    createdAt: string;
    active: boolean;
    onOpen: () => void;
  }> = [
    ...history.map((c) => ({
      id: c.id,
      title: c.title,
      createdAt: c.createdAt,
      active: c.id === currentId,
      onOpen: c.id === currentId ? () => setHistoryOpen(false) : () => openConversation(c)
    })),
    ...serverThreads
      .filter((t) => !history.some((c) => c.id === t.id))
      .map((t) => ({
        id: t.id,
        title: t.title,
        createdAt: t.created_at,
        active: t.id === currentId,
        onOpen: t.id === currentId ? () => setHistoryOpen(false) : () => void openServerThread(t.id)
      }))
  ];
  // A brand-new chat isn't in the server list yet — surface it so the active
  // conversation is always present (and highlightable).
  if (!empty && currentId != null && !historyEntries.some((e) => e.id === currentId)) {
    historyEntries.push({
      id: currentId,
      title: turns[0]?.question ?? pending ?? currentId,
      createdAt: startedAtRef.current ?? new Date().toISOString(),
      active: true,
      onOpen: () => setHistoryOpen(false)
    });
  }
  // Order strictly by creation time (newest first) — not by which chat was
  // last opened. ISO-8601 strings sort chronologically as plain text.
  historyEntries.sort((a, b) => b.createdAt.localeCompare(a.createdAt));

  // Search filter, then reveal up to `historyShown` rows ("Show more").
  const q = historyQuery.trim().toLowerCase();
  const filteredEntries = q
    ? historyEntries.filter((e) => e.title.toLowerCase().includes(q))
    : historyEntries;
  const visibleEntries = filteredEntries.slice(0, historyShown);

  const placeholder =
    run.state === "needs_clarification"
      ? "Reply…"
      : pending !== null
        ? "Ask a follow-up…"
        : `Ask Oxygen anything about ${workspaceName ?? "your workspace"}…`;

  return (
    <aside
      className={cx("oxy-shell-scope oxy-askdock", !open && "oxy-askdock--closed", className)}
      aria-label='Ask Oxygen'
      aria-hidden={!open}
    >
      <header className='oxy-askdock__head'>
        <OxyMark className='oxy-askdock__mark' />
        <span className='oxy-askdock__title'>Ask Oxygen</span>
        <span className='oxy-askdock__actions'>
          <button
            type='button'
            className='oxy-askdock__iconbtn'
            onClick={newChat}
            disabled={!canNewChat}
            data-testid='askdock-new-chat'
            aria-label='New chat'
            title='New chat'
          >
            <PlusIcon />
          </button>
          <button
            type='button'
            className={cx("oxy-askdock__iconbtn", historyOpen && "oxy-askdock__iconbtn--active")}
            onClick={() => {
              const next = !historyOpen;
              setHistoryOpen(next);
              if (next) {
                refetchHistory();
                setHistoryQuery("");
                setHistoryShown(HISTORY_PAGE_SIZE);
              }
            }}
            data-testid='askdock-history'
            aria-label='Chat history'
            aria-expanded={historyOpen}
            title='Chat history'
          >
            <HistoryIcon />
          </button>
          {threadsHref && (
            <a
              className='oxy-askdock__iconbtn'
              href={threadsHref}
              aria-label='Open chat in Oxygen'
              title='Open chat in Oxygen'
            >
              <ExternalIcon />
            </a>
          )}
          <button
            type='button'
            className='oxy-askdock__iconbtn'
            onClick={onClose}
            aria-label='Close Ask Oxygen'
          >
            <CloseIcon />
          </button>
        </span>
      </header>

      {historyOpen ? (
        <div className='oxy-askdock__history' data-testid='askdock-history-panel'>
          <div className='oxy-askdock__history-search'>
            <SearchIcon />
            <input
              type='text'
              className='oxy-askdock__history-searchinput'
              placeholder='Search threads…'
              value={historyQuery}
              onChange={(e) => {
                setHistoryQuery(e.target.value);
                setHistoryShown(HISTORY_PAGE_SIZE);
              }}
              aria-label='Search threads'
            />
          </div>
          {visibleEntries.length === 0 ? (
            <p className='oxy-askdock__history-empty'>
              {historyQuery ? "No matching chats." : "No previous chats yet."}
            </p>
          ) : (
            <>
              {visibleEntries.map((entry) => (
                <button
                  key={entry.id}
                  type='button'
                  className={cx(
                    "oxy-askdock__history-item",
                    entry.active && "oxy-askdock__history-item--active"
                  )}
                  onClick={entry.onOpen}
                  title={entry.title}
                  aria-current={entry.active ? "true" : undefined}
                >
                  <span className='oxy-askdock__history-icon'>
                    <ThreadIcon />
                  </span>
                  <span className='oxy-askdock__history-title'>{entry.title}</span>
                  <span className='oxy-askdock__history-time'>
                    {formatHistoryTime(entry.createdAt)}
                  </span>
                </button>
              ))}
              {filteredEntries.length > visibleEntries.length && (
                <button
                  type='button'
                  className='oxy-askdock__history-more'
                  onClick={() => setHistoryShown((n) => n + HISTORY_PAGE_SIZE)}
                >
                  Show more
                </button>
              )}
            </>
          )}
        </div>
      ) : (
        <>
          <div className='oxy-askdock__scroll' ref={scrollRef}>
            {restoring && <p className='oxy-askdock__history-empty'>Loading conversation…</p>}
            {empty && !restoring && suggestions.length > 0 && (
              <div className='oxy-askdock__chips'>
                {suggestions.map((s) => (
                  <button
                    key={s}
                    type='button'
                    className='oxy-askdock__chip'
                    onClick={() => send(s)}
                  >
                    {s}
                  </button>
                ))}
              </div>
            )}
            {turns.map((t, i) => (
              <div key={`${i}-${t.question}`} className='oxy-askdock__turn'>
                <div className='oxy-askdock__q'>{t.question}</div>
                <ReasoningTrace steps={t.steps} llm={t.llm} />
                {t.charts.map((c, ci) => (
                  <AnswerChart key={`${ci}-${c.config.title ?? c.config.chart_type}`} block={c} />
                ))}
                {/* No artifacts prop: the SQL details live in the reasoning
                    trace and the chart — the QUERY block would triple up. */}
                <OxyAnswer answer={t.answer} state='done' threadUrl={null} />
              </div>
            ))}
            {pending !== null && (
              <div className='oxy-askdock__turn'>
                <div className='oxy-askdock__q'>{pending}</div>
                <ReasoningTrace steps={liveSteps} streaming={busy} llm={liveLlm} />
                {liveCharts.map((c, ci) => (
                  <AnswerChart key={`${ci}-${c.config.title ?? c.config.chart_type}`} block={c} />
                ))}
                <OxyAnswer
                  answer={run.answer}
                  state={run.state}
                  clarification={run.clarification}
                  error={run.error}
                  threadUrl={null}
                />
              </div>
            )}
          </div>

          <form
            className='oxy-askdock__composer'
            onSubmit={(e) => {
              e.preventDefault();
              send(draft);
            }}
          >
            <textarea
              ref={inputRef}
              className='oxy-askdock__input'
              rows={3}
              placeholder={placeholder}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  send(draft);
                }
              }}
            />
            <div className='oxy-askdock__composer-row'>
              {busy ? (
                <button
                  type='button'
                  className='oxy-askdock__send'
                  onClick={run.cancel}
                  aria-label='Stop generating'
                >
                  <StopIcon />
                </button>
              ) : (
                <button
                  type='submit'
                  className='oxy-askdock__send'
                  disabled={!draft.trim()}
                  aria-label='Send'
                >
                  <ArrowUpIcon />
                </button>
              )}
            </div>
          </form>
        </>
      )}
    </aside>
  );
}
