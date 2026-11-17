// The shell's one Light / Dark / System switch, in the same place in every app: the rail's bottom
// slot (where Oxygen keeps its user menu), and the nav sheet's bottom on a phone. `OxyShell` draws
// it only with theme.css applied, for a signed-in member on a server that carries the mode, and
// with no pin (`mode.ts`). The rail form is one button opening a three-item menu; the sheet form is
// a three-way control, since a phone has the width and no hover.

import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import type { OxyModeChoice } from "../theme/index";
import { cx } from "./cx";
import { ShellTooltip } from "./Tooltip";

const OPTIONS: { value: OxyModeChoice; label: string }[] = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "system", label: "System" }
];

/** Sun, moon and monitor (lucide outlines, inlined). */
function ModeIcon({ mode }: { mode: OxyModeChoice }) {
  return (
    <svg
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
      {mode === "light" && (
        <>
          <circle cx='12' cy='12' r='4' />
          <path d='M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41' />
        </>
      )}
      {mode === "dark" && <path d='M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z' />}
      {mode === "system" && (
        <>
          <rect width='20' height='14' x='2' y='3' rx='2' />
          <path d='M8 21h8M12 17v4' />
        </>
      )}
    </svg>
  );
}

export interface ModeSwitchProps {
  /** The person's choice. */
  mode: OxyModeChoice;
  onChoose: (mode: OxyModeChoice) => void;
  /** `rail`: a button and its menu. `sheet`: a three-way control with labels. */
  variant: "rail" | "sheet";
}

/** One switch, two forms. See the file header. */
export function ModeSwitch({ mode, onChoose, variant }: ModeSwitchProps) {
  return variant === "rail" ? (
    <RailModeSwitch mode={mode} onChoose={onChoose} />
  ) : (
    <SheetModeSwitch mode={mode} onChoose={onChoose} />
  );
}

function SheetModeSwitch({ mode, onChoose }: Omit<ModeSwitchProps, "variant">) {
  return (
    <div
      role='radiogroup'
      aria-label='Appearance'
      data-testid='shell-mode-switch'
      className='oxy-modeswitch-seg'
    >
      {OPTIONS.map(({ value, label }) => (
        // biome-ignore lint/a11y/useSemanticElements: a segmented control, not a form's radio inputs
        <button
          key={value}
          type='button'
          role='radio'
          aria-checked={mode === value}
          data-testid={`shell-mode-${value}`}
          className={cx(
            "oxy-modeswitch-seg__option",
            mode === value && "oxy-modeswitch-seg__option--on"
          )}
          onClick={() => onChoose(value)}
        >
          <ModeIcon mode={value} />
          <span>{label}</span>
        </button>
      ))}
    </div>
  );
}

function RailModeSwitch({ mode, onChoose }: Omit<ModeSwitchProps, "variant">) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const items = useRef<(HTMLButtonElement | null)[]>([]);
  const current = OPTIONS.find((o) => o.value === mode) ?? OPTIONS[0];

  // Open: the checked item takes focus. Outside press or Escape closes it.
  useEffect(() => {
    if (!open) return;
    items.current[OPTIONS.indexOf(current)]?.focus();
    const onDown = (e: PointerEvent) => {
      if (wrap.current && !wrap.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [open, current]);

  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  const onMenuKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const at = items.current.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : OPTIONS.length - 1;
      items.current[(Math.max(at, 0) + step) % OPTIONS.length]?.focus();
    } else if (e.key === "Tab") {
      setOpen(false);
    }
  };

  return (
    <div ref={wrap} className='oxy-modeswitch'>
      <ShellTooltip content={`Appearance: ${current.label}`}>
        <button
          ref={trigger}
          type='button'
          data-testid='shell-mode-switch'
          aria-label={`Appearance: ${current.label}`}
          aria-haspopup='menu'
          aria-expanded={open}
          className={cx("oxy-rail__item", open && "oxy-rail__item--active")}
          onClick={() => setOpen((o) => !o)}
        >
          <ModeIcon mode={mode} />
        </button>
      </ShellTooltip>
      {open && (
        <div
          role='menu'
          aria-label='Appearance'
          className='oxy-shell-scope oxy-modeswitch__menu'
          onKeyDown={onMenuKey}
        >
          {OPTIONS.map(({ value, label }, i) => (
            <button
              key={value}
              ref={(el) => {
                items.current[i] = el;
              }}
              type='button'
              role='menuitemradio'
              aria-checked={mode === value}
              data-testid={`shell-mode-${value}`}
              className='oxy-modeswitch__item'
              onClick={() => {
                onChoose(value);
                close();
              }}
            >
              <ModeIcon mode={value} />
              <span className='oxy-modeswitch__label'>{label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
