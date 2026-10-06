import type { EChartsOption } from "echarts";
import MiniChart from "./MiniChart";

interface ChartCardProps {
  /** What the figure counts. Also names the chart if it fails to draw. */
  label: string;
  value: string | number;
  options: EChartsOption;
  isLoading: boolean;
}

/**
 * One chart of the band: what is counted, the figure, then its shape over
 * time. Label and figure are separate lines so the four figures sit on one
 * baseline and can be read across.
 */
export default function ChartCard({ label, value, options, isLoading }: ChartCardProps) {
  return (
    <div className='flex min-w-0 flex-col gap-2 p-4'>
      <div className='min-w-0'>
        <p className='t-label truncate text-muted-foreground'>{label}</p>
        <p className='t-h2 truncate tabular-nums'>{value}</p>
      </div>
      <MiniChart options={options} isLoading={isLoading} title={label} />
    </div>
  );
}
