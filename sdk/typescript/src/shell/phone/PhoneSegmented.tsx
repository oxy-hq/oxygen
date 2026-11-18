import { type KeyboardEvent, useLayoutEffect, useRef } from "react";
import { cx } from "../cx";
import { edgesToFade, scrollToShow } from "./segmentedScroll";

export interface PhoneSegmentedOption<T extends string = string> {
  value: T;
  label: string;
}

export interface PhoneSegmentedProps<T extends string = string> {
  /** Two to four; more belong in a select or a sheet. */
  options: PhoneSegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  /** The group's name, read by screen readers ("View"). */
  ariaLabel?: string;
}

/** Equal 44px segments for a small switch: List | Map, Day | Week | Month.
 *  A radio group: one tab stop, arrow keys move the choice. A label too long for
 *  an equal share keeps its whole width, and a row still too wide for the screen
 *  scrolls sideways, faded at an edge with more beyond it, the selected option
 *  kept in view: no label is cut. */
export function PhoneSegmented<T extends string = string>({
  options,
  value,
  onChange,
  ariaLabel
}: PhoneSegmentedProps<T>) {
  const groupRef = useRef<HTMLDivElement>(null);
  const placed = useRef(false);
  const labels = options.map((o) => o.label).join("\n");
  const selected = options.findIndex((o) => o.value === value);

  // The edges to fade, measured again on a scroll and whenever the row or an
  // option changes size (a font arriving, a new label). The mark is also what
  // makes the row scroll (shell.css), so a row that fits never becomes a scroll
  // container. Before paint, so an overflowing row is never drawn spilling out.
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-runs for new labels, which are new options to watch.
  useLayoutEffect(() => {
    const group = groupRef.current;
    if (!group) return;
    const mark = () => {
      const fade = edgesToFade(group);
      if (fade) group.setAttribute("data-oxy-fade", fade);
      else group.removeAttribute("data-oxy-fade");
    };
    mark();
    group.addEventListener("scroll", mark, { passive: true });
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(mark);
    if (observer) {
      observer.observe(group);
      for (const option of group.children) observer.observe(option);
    }
    return () => {
      group.removeEventListener("scroll", mark);
      observer?.disconnect();
    };
  }, [labels]);

  // The selected option stays wholly in view, clear of the fades: at once as the
  // row first draws, smoothly when the choice moves.
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-runs for new labels, which move the options.
  useLayoutEffect(() => {
    const group = groupRef.current;
    const option = group?.querySelectorAll<HTMLElement>("button")[selected];
    if (!group || !option) return;
    const left = scrollToShow(group, option);
    if (left !== null) {
      const smooth = placed.current && !prefersReducedMotion();
      group.scrollTo({ left, behavior: smooth ? "smooth" : "auto" });
    }
    placed.current = true;
  }, [selected, labels]);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
    if (!step || options.length === 0) return;
    e.preventDefault();
    const at = options.findIndex((o) => o.value === value);
    // Nothing chosen yet: the first arrow lands on an end, not one past it.
    const next =
      at < 0 ? (step > 0 ? 0 : options.length - 1) : (at + step + options.length) % options.length;
    onChange(options[next].value);
    groupRef.current?.querySelectorAll<HTMLButtonElement>("button")[next]?.focus();
  };

  // One tab stop always, even while `value` matches no option (still loading).
  const tabStop = options.some((o) => o.value === value) ? value : options[0]?.value;

  return (
    <div
      ref={groupRef}
      role='radiogroup'
      aria-label={ariaLabel}
      className='oxy-shell-scope oxy-phone-segmented'
      onKeyDown={onKeyDown}
    >
      {options.map((option) => {
        const checked = option.value === value;
        return (
          // biome-ignore lint/a11y/useSemanticElements: a segmented control is a button row styled as one control; a native radio cannot take this shape.
          <button
            key={option.value}
            type='button'
            role='radio'
            aria-checked={checked}
            tabIndex={option.value === tabStop ? 0 : -1}
            className={cx(
              "oxy-phone-segmented__option",
              checked && "oxy-phone-segmented__option--active"
            )}
            onClick={() => onChange(option.value)}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

function prefersReducedMotion(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}
