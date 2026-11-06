import type { ReactNode } from "react";
import { cx } from "../cx";
import { present, toneClass } from "./tone";

export interface PhoneKpi {
  label: string;
  value: ReactNode;
  /** e.g. "+8.9% vs LY", coloured by `tone`. */
  delta?: ReactNode;
  tone?: "good" | "bad" | "neutral";
  /** Makes the tile a button (e.g. scroll to the section behind the number). */
  onClick?: () => void;
}

export interface PhoneKpiStripProps {
  items: PhoneKpi[];
}

function KpiBody({ item }: { item: PhoneKpi }) {
  return (
    <>
      <span className='oxy-phone-kpi__label'>{item.label}</span>
      <span className='oxy-phone-kpi__value'>{item.value}</span>
      {present(item.delta) && (
        <span className={cx("oxy-phone-kpi__delta", toneClass(item.tone ?? "neutral"))}>
          {item.delta}
        </span>
      )}
    </>
  );
}

/** The headline numbers, two to a row, on the first screen. */
export function PhoneKpiStrip({ items }: PhoneKpiStripProps) {
  return (
    <div className='oxy-shell-scope oxy-phone-kpis'>
      {items.map((item) =>
        item.onClick ? (
          <button
            key={item.label}
            type='button'
            className='oxy-phone-kpi oxy-phone-kpi--button'
            onClick={item.onClick}
          >
            <KpiBody item={item} />
          </button>
        ) : (
          <div key={item.label} className='oxy-phone-kpi'>
            <KpiBody item={item} />
          </div>
        )
      )}
    </div>
  );
}
