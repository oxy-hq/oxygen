import { COLUMN_LABELS } from "../dataFlowModel";

const LINES: { label: string; stroke: string; dash?: string }[] = [
  { label: "reads / writes", stroke: "var(--muted-foreground)" },
  { label: "triggers", stroke: "var(--muted-foreground)", dash: "6 4" },
  { label: "ships (change stream)", stroke: "var(--primary)", dash: "2 3" },
  { label: "upload not approved", stroke: "var(--warning)", dash: "6 4" }
];

/** The columns, left to right, and what each kind of line means. */
export const DataFlowLegend = () => (
  <div
    className='flex flex-wrap items-center gap-x-4 gap-y-1 text-[10px] text-muted-foreground'
    data-testid='admin-data-flow-legend'
  >
    <span>{COLUMN_LABELS.join(" → ")}</span>
    {LINES.map((line) => (
      <span key={line.label} className='flex items-center gap-1.5'>
        <svg width='24' height='6' aria-hidden='true'>
          <line
            x1='0'
            y1='3'
            x2='24'
            y2='3'
            stroke={line.stroke}
            strokeWidth='1.5'
            strokeDasharray={line.dash}
          />
        </svg>
        {line.label}
      </span>
    ))}
  </div>
);
