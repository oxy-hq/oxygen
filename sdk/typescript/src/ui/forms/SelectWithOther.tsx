// Started from pokehouse-oxy `apps/warehouse/src/components/SelectWithOther.tsx` (INVENTORY
// section 2: the only copy, tested by `field.test.ts`), itself drawn as the reference's
// (oxy-hq/reference-app@3caad1f `src/components/SelectWithOther.tsx`). Kept whole: the props,
// `OTHER`, the comfortable select, the sentinel in the placeholder's ink while it is the choice,
// the free-text box named for what it is the free-text version of. The app's option lists were
// already an `options` prop; its `SelectOption` type is `SelectWithOtherOption` here (one name
// across every group of the entry).
//
// A dropdown with a leading sentinel ("Not known", "Select…"), the real options, and a trailing
// "Other…" that reveals a free-text input. Fully controlled: `choice` and `otherText` come in,
// `onChange(choice, otherText)` goes out, and the typed text survives a trip away from Other and
// back, because the field never owns it.

import { cx } from "../cx";
import { ui } from "../recipes";

/** Reserved choice value for the trailing free-text option. */
export const OTHER = "__other";

export interface SelectWithOtherOption {
  value: string;
  label: string;
}

export interface SelectWithOtherProps {
  id?: string;
  /** For a select no <label> points at, such as a repeated row's. */
  ariaLabel?: string;
  options: readonly SelectWithOtherOption[];
  /** Leading sentinel option (always first). Still a real answer: it only reads as a placeholder. */
  leading: SelectWithOtherOption;
  /** The trailing option's label. Default "Other…". */
  otherLabel?: string;
  choice: string;
  otherText: string;
  /** The free-text box's placeholder, and its accessible name. */
  otherPlaceholder?: string;
  onChange: (choice: string, otherText: string) => void;
  invalid?: boolean;
  /** The id of the field's message, from `describedBy` (or a Field's function child). */
  describedBy?: string;
  disabled?: boolean;
  className?: string;
}

export function SelectWithOther({
  id,
  ariaLabel,
  options,
  leading,
  otherLabel,
  choice,
  otherText,
  otherPlaceholder,
  onChange,
  invalid,
  describedBy,
  disabled,
  className
}: SelectWithOtherProps) {
  return (
    <div className={cx("oxy-ui-select-with-other", className)}>
      <select
        id={id}
        aria-label={ariaLabel}
        aria-describedby={describedBy}
        aria-invalid={invalid ? true : undefined}
        className={cx(
          ui.selectTouch,
          "oxy-ui-select-with-other__select",
          // The sentinel reads as a placeholder while it is the choice; only the closed field
          // greys, each option keeps full ink (forms.css).
          choice === leading.value && ui.selectPlaceholder,
          invalid && ui.errorInput
        )}
        value={choice}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value, otherText)}
      >
        <option value={leading.value}>{leading.label}</option>
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
        <option value={OTHER}>{otherLabel ?? "Other…"}</option>
      </select>
      {choice === OTHER && (
        <input
          type='text'
          // Named for what it is the free-text version of; without it the second box is read
          // as an unlabelled text input.
          aria-label={otherPlaceholder}
          aria-describedby={describedBy}
          aria-invalid={invalid ? true : undefined}
          className={cx(ui.input, invalid && ui.errorInput)}
          value={otherText}
          placeholder={otherPlaceholder}
          disabled={disabled}
          onChange={(e) => onChange(OTHER, e.target.value)}
        />
      )}
    </div>
  );
}
