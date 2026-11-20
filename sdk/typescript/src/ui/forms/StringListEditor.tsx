// Started from pokehouse-oxy `apps/warehouse/src/components/StringListEditor.tsx` (INVENTORY
// section 2: the copy `tapTargets.test.ts` covers; bookkeeping's is the other), drawn as the
// reference's (oxy-hq/reference-app@3caad1f `src/components/StringListEditor.tsx`). Kept: the
// props, neutral removable chips whose × names the entry, the comfortable input and its Add,
// Enter to add, a case-insensitive duplicate dropping out, `validateEntry` as the fast path,
// the draft resyncing when the SAVED values change, Save (warehouse's: its lists persist) as the
// screen's 44px primary with "Unsaved changes" beside it. Changed: Save is a busy Button while it
// runs (focus stays on it; the app's disabled Save dropped focus to <body>), and removing an
// entry hands focus to the next entry's ×, the one before, or the input, instead of to <body>.
// `keepOn` is the reference's: what is typed and not yet saved survives the swap between the
// app's phone and desk layouts, through the SDK's `useKeptState` (the reference kept the
// half-typed entry; this editor also holds a list not yet saved, so that is kept too).
//
// One editable list of words: shipping companies, email addresses, codes.

import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { useKeptState } from "../../shell/keptState";
import { Button } from "../actions";
import { cx } from "../cx";
import { Chip } from "../figures";
import { PlusIcon } from "../icons";
import { ui } from "../recipes";

export interface StringListEditorProps {
  /** What the entries ARE ("Shipping companies"), for the input's and each ×'s name. */
  label: string;
  /** The saved list. A change resyncs the draft. */
  values: readonly string[];
  placeholder: string;
  /** Saves the draft. Resolves to an error to show, or null. */
  onSave: (values: string[]) => Promise<string | null>;
  /** The fast path for one entry: an error to show, or null. The server still validates. */
  validateEntry?: (entry: string) => string | null;
  disabled?: boolean;
  /**
   * The screen (its path) whose kept state holds what is typed but not saved, the half-typed
   * entry and the unsaved list, across the phone/desk swap (`useKeptState` under the app's
   * `KeepAcrossLayouts`, from `@oxy-hq/sdk/shell`), keyed by `label`: the reference's `keepOn`.
   * Unset (a specimen with no screen), the editor keeps them under a key of its own mount.
   */
  keepOn?: string;
  className?: string;
}

/** The list being edited, and the saved list it started from (a change to that resyncs it). */
interface Draft {
  base: string;
  list: string[];
}

// Joins a list into one comparable key. NUL never appears in an entry.
const SEP = "\u0000";

export function StringListEditor({
  label,
  values,
  placeholder,
  onSave,
  validateEntry,
  disabled = false,
  keepOn,
  className
}: StringListEditorProps) {
  const savedKey = values.join(SEP);
  const mount = useId();
  const keyOf = (field: string) => (keepOn ? `${keepOn}#${field}:${label}` : `${mount}#${field}`);
  const [kept, setKept] = useKeptState<Draft>(keyOf("draft"), () => ({
    base: savedKey,
    list: [...values]
  }));
  const draft = kept.list;
  const setDraft = (list: string[]) => setKept({ base: savedKey, list });
  const [entry, setEntry] = useKeptState(keyOf("entry"), "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const saveRef = useRef<HTMLButtonElement>(null);
  const refocus = useRef<number | null>(null);
  // Set when a save succeeds from the Save button: the input takes focus once it is enabled again.
  const refocusInput = useRef(false);
  const noun = label.toLowerCase();

  // Resync the draft whenever the SAVED values change (after a save, a refresh), and not when the
  // other layout's mount takes over a draft made against the same saved list.
  // biome-ignore lint/correctness/useExhaustiveDependencies: savedKey encodes `values`
  useEffect(() => {
    if (kept.base !== savedKey) setKept({ base: savedKey, list: [...values] });
  }, [savedKey]);

  // After a remove, focus the entry that took the removed one's place, the one before, or the
  // input; after a save, the input.
  useLayoutEffect(() => {
    if (refocusInput.current && !saving) {
      refocusInput.current = false;
      inputRef.current?.focus();
    }
    const at = refocus.current;
    if (at === null) return;
    refocus.current = null;
    const removes = [...(listRef.current?.querySelectorAll<HTMLElement>("button") ?? [])];
    (removes[Math.min(at, removes.length - 1)] ?? inputRef.current)?.focus();
  });

  const dirty = draft.join(SEP) !== savedKey;
  const locked = disabled || saving;

  const add = () => {
    const v = entry.trim();
    if (!v) return;
    const entryError = validateEntry?.(v) ?? null;
    if (entryError) {
      setError(entryError);
      return;
    }
    if (!draft.some((d) => d.toLowerCase() === v.toLowerCase())) setDraft([...draft, v]);
    setEntry("");
    setError(null);
  };

  const remove = (index: number) => {
    refocus.current = index;
    setDraft(draft.filter((_, i) => i !== index));
  };

  const saveNow = async () => {
    if (saving) return;
    setSaving(true);
    setError(null);
    const err = await onSave(draft);
    setSaving(false);
    if (err) setError(err);
    // Saved: the Save row goes once the list resyncs, so focus moves on to the input.
    else if (document.activeElement === saveRef.current) refocusInput.current = true;
  };

  return (
    <div className={cx("oxy-ui-string-list", className)}>
      {draft.length > 0 && (
        <ul ref={listRef} className='oxy-ui-string-list__entries'>
          {draft.map((v, i) => (
            <li key={v}>
              <Chip
                tone='neutral'
                onRemove={() => remove(i)}
                // Names the entry, not the action: a list of twelve is not "Remove, Remove, …".
                removeLabel={`Remove ${v} from ${noun}`}
                disabled={locked}
              >
                {v}
              </Chip>
            </li>
          ))}
        </ul>
      )}
      {/* The reference's row: the 44px input and the compact Add, which the row's stretch draws
          at the input's height. */}
      <div className='oxy-ui-string-list__row'>
        <input
          ref={inputRef}
          type='text'
          className={cx(ui.input, "oxy-ui-string-list__input", error && ui.errorInput)}
          value={entry}
          placeholder={placeholder}
          aria-label={`Add to ${noun}`}
          aria-invalid={error ? true : undefined}
          disabled={locked}
          onChange={(e) => {
            setEntry(e.target.value);
            if (error) setError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              add();
            }
          }}
        />
        <Button onClick={add} disabled={locked || !entry.trim()} icon={<PlusIcon />}>
          Add
        </Button>
      </div>
      {error && (
        <p role='alert' className='oxy-ui-string-list__error'>
          {error}
        </p>
      )}
      {dirty && (
        <div className='oxy-ui-string-list__save'>
          <Button
            ref={saveRef}
            variant='primary'
            size='lg'
            onClick={saveNow}
            disabled={disabled}
            busy={saving}
            busyLabel='Saving…'
          >
            Save
          </Button>
          {!saving && !error && (
            <span className={cx(ui.toneText.warning, "oxy-ui-string-list__unsaved")}>
              Unsaved changes
            </span>
          )}
        </div>
      )}
    </div>
  );
}
