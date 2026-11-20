// New: BEYOND-GALLERY row 8 (a search box in 13 files across 4 repos, each with its own filter;
// a clear control in 3, Escape in about 6, `type="search"` skipped by the staging demos). The
// spec is the reference's recipes (oxy-hq/reference-app@3caad1f `src/components/ui.ts:1125-1155`,
// `inputSm` and `inputQuiet`: the magnifier, the placeholder naming what to type,
// `type="search"`, the edge arriving on hover and focus) and its toolbar search
// (`src/components/ReorderSheet.tsx:164-177`). New with it: a clear button, Escape to clear, and
// `matchesQuery`, the one filter rule every copy approximated.
//
// A field that filters a list or a table as you type. Controlled: `value` in, `onChange(next)`
// out; the list filters itself with `matchesQuery(row text, value)`.
//
// **Size.** `sm` (default) is the 32px find field in a desk toolbar, beside 32px buttons; `lg`
// the 44px field of an entry screen. `quiet` drops the edge until a pointer or a keyboard goes
// looking (the reference's catalogue search at the head of a table); it keeps the magnifier and
// the placeholder, which are what say "a control" (1.4.11). Never quiet for a field to fill in.
//
// **The magnifier** sits where the reference draws it: at the end of the `sm` find (the
// reference's ReorderSheet toolbar: `right-3`, the input's `pr-9`), at the start of the `lg`
// field (its WarehouseReports and ItemPickerSheet finds: `left-4`, `pl-11`).
//
// **Clearing.** The × shows while there is text (on `sm`, in the magnifier's place), clears it,
// and gives focus back to the field.
// Escape in a field with text clears it and goes no further; in an empty field it goes on up,
// so the dialog around a search still closes on Escape.

import { type ComponentPropsWithRef, type KeyboardEvent, type Ref, useRef } from "react";
import { cx } from "../cx";
import { XIcon } from "../icons";
import { ui } from "../recipes";
import { MagnifyingGlassIcon } from "./icons";

/**
 * The one filter rule: `query` matches when every word of it, trimmed and lowercased, appears
 * somewhere in `text` (a string, or several fields read as one). An empty query matches all.
 *
 * @example matchesQuery(["Cascade Freight", "CF-201"], "  cf freight ") // true
 */
export function matchesQuery(
  text: string | null | undefined | readonly (string | null | undefined)[],
  query: string
): boolean {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const hay = (typeof text === "string" || text == null ? [text] : text)
    .filter((t): t is string => typeof t === "string")
    .join(" ")
    .toLowerCase();
  return words.every((w) => hay.includes(w));
}

export interface SearchFieldProps
  extends Omit<
    ComponentPropsWithRef<"input">,
    "value" | "onChange" | "type" | "size" | "children" | "className"
  > {
  value: string;
  onChange: (value: string) => void;
  /** The field's accessible name ("Search SKU or item"), and its placeholder unless one is given. */
  label: string;
  /** `sm` 32px (a desk toolbar, default), `lg` 44px (an entry screen). */
  size?: "sm" | "lg";
  /** No edge until hovered or focused. */
  quiet?: boolean;
  /** The clear button's name. Default "Clear search". */
  clearLabel?: string;
  /** On the wrapper, for placement (a toolbar's `max-width`). */
  className?: string;
}

function setRef<T>(ref: Ref<T> | undefined, value: T | null) {
  if (typeof ref === "function") ref(value);
  else if (ref) (ref as { current: T | null }).current = value;
}

export function SearchField({
  value,
  onChange,
  label,
  size = "sm",
  quiet = false,
  clearLabel = "Clear search",
  placeholder,
  className,
  ref,
  onKeyDown,
  ...rest
}: SearchFieldProps) {
  const input = useRef<HTMLInputElement | null>(null);
  const clear = () => {
    onChange("");
    input.current?.focus();
  };
  const keyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    onKeyDown?.(event);
    if (event.defaultPrevented || event.key !== "Escape" || value === "") return;
    // Text to clear: this Escape is the field's, not the dialog's around it.
    event.preventDefault();
    event.stopPropagation();
    onChange("");
  };
  return (
    <div
      className={cx("oxy-ui-search-field", `oxy-ui-search-field--${size}`, className)}
      data-filled={value === "" ? undefined : ""}
    >
      <MagnifyingGlassIcon className={cx(ui.iconControl, "oxy-ui-search-field__glass")} />
      <input
        {...rest}
        ref={(el) => {
          input.current = el;
          setRef(ref, el);
        }}
        type='search'
        aria-label={label}
        placeholder={placeholder ?? label}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={keyDown}
        className={cx(
          size === "sm" ? ui.inputSm : ui.input,
          quiet && ui.fieldQuiet,
          "oxy-ui-search-field__input"
        )}
      />
      {value !== "" && (
        <button
          type='button'
          className={cx("oxy-ui-search-field__clear", ui.focusRing)}
          aria-label={clearLabel}
          onClick={clear}
          disabled={rest.disabled}
        >
          <XIcon className={ui.iconInline} />
        </button>
      )}
    </div>
  );
}
