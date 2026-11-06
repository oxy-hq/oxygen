import { cx } from "../cx";
import { ChevronDownIcon, FilterIcon } from "./icons";

export interface PhoneFilterChipProps {
  /** What is being shown: "Oct 2026 · All companies". */
  label: string;
  /** Filters set away from their default; 0 or none shows no count. */
  count?: number;
  /** Open the app's filters `PhoneSheet`. */
  onClick: () => void;
}

/** The one filter control on a phone page, in place of a row of selects. */
export function PhoneFilterChip({ label, count, onClick }: PhoneFilterChipProps) {
  const active = count !== undefined && count > 0;
  return (
    <button
      type='button'
      aria-haspopup='dialog'
      className={cx("oxy-shell-scope oxy-phone-chip", active && "oxy-phone-chip--active")}
      onClick={onClick}
    >
      <FilterIcon />
      <span className='oxy-phone-chip__label'>{label}</span>
      {active && (
        <span className='oxy-phone-chip__count'>
          {count}
          <span className='oxy-phone-sr'>{count === 1 ? " filter set" : " filters set"}</span>
        </span>
      )}
      <ChevronDownIcon />
    </button>
  );
}
