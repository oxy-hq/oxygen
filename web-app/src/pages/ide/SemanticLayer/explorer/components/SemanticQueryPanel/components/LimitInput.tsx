import { useEffect, useState } from "react";

/**
 * Row-cap control for the semantic explorer.
 *
 * Holds its own draft string rather than binding straight to the numeric
 * `limit`. A controlled `value={limit}` with a `v > 0` guard cannot be
 * cleared: deleting the last digit produces `""`, `Number("")` is `0`, the
 * guard rejects it, and the field snaps back to the old number — so typing
 * `500` over `1000` is impossible without selecting the text first.
 *
 * The draft is the only thing that reflects keystrokes; `onLimitChange` fires
 * only for a value that is actually a positive number, and an empty or
 * invalid field falls back to the last committed limit on blur rather than
 * committing a `0` nobody asked for.
 */
const LimitInput = ({
  limit,
  onLimitChange
}: {
  limit: number;
  onLimitChange: (limit: number) => void;
}) => {
  const [draft, setDraft] = useState(String(limit));

  // Follow the limit when something else changes it (a saved query loading,
  // a reset) without fighting the user mid-edit: only when it disagrees with
  // what the draft already means.
  useEffect(() => {
    setDraft((current) => (Number(current) === limit ? current : String(limit)));
  }, [limit]);

  return (
    <div className='flex shrink-0 items-center gap-1'>
      <span className='text-muted-foreground text-xs'>Limit</span>
      <input
        type='number'
        min={1}
        value={draft}
        aria-label='Row limit'
        onChange={(e) => {
          setDraft(e.target.value);
          const v = Number(e.target.value);
          if (e.target.value !== "" && Number.isFinite(v) && v > 0) onLimitChange(v);
        }}
        onBlur={() => setDraft(String(limit))}
        className='h-7 w-20 rounded-md border bg-background px-2 text-sm'
      />
    </div>
  );
};

export default LimitInput;
