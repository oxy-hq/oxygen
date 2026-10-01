import type { ECElementEvent, EChartsOption } from "echarts";
import { getInstanceByDom, init } from "echarts";
import { useCallback, useEffect, useRef } from "react";
import { ErrorBoundary } from "react-error-boundary";
import { useResizeDetector } from "react-resize-detector";
import theme from "@/components/Echarts/theme.json";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { cn } from "@/libs/shadcn/utils";

/**
 * Map of ECharts event name → handler. Pass a memoized object (useMemo /
 * useCallback) so handlers are not rebound on every render.
 */
type EChartEventHandlers = Record<string, (params: ECElementEvent) => void>;

interface EChartProps {
  /** The chart option. Rebuild (memoize) it when data changes — it drives `setOption`. */
  option: EChartsOption;
  /** Chart height in px. Defaults to 220. */
  height?: number;
  className?: string;
  /** ECharts events, e.g. `{ click: (p) => ... }`. Keep the object referentially stable. */
  onEvents?: EChartEventHandlers;
  /** Show ECharts' built-in loading spinner. */
  loading?: boolean;
}

/**
 * Shared ECharts host. Encapsulates the init / getInstanceByDom / setOption /
 * resize-observer / dispose lifecycle plus the repo `theme.json`, wrapped in a
 * `react-error-boundary` so a bad option never takes down the surrounding page.
 *
 * This is the one place obs (and other) charts should mount ECharts — do not
 * hand-roll another `echarts.init` block.
 */
function EChartInner({ option, height = 220, className, onEvents, loading }: EChartProps) {
  const chartRef = useRef<HTMLDivElement>(null);

  const onResize = useCallback(() => {
    if (chartRef.current) {
      getInstanceByDom(chartRef.current)?.resize();
    }
  }, []);

  useResizeDetector({ targetRef: chartRef, onResize });

  // Init once; dispose on unmount.
  useEffect(() => {
    if (!chartRef.current) return;
    const chart = init(chartRef.current, theme);
    return () => {
      chart.dispose();
    };
  }, []);

  // Push option whenever it changes.
  useEffect(() => {
    if (!chartRef.current) return;
    const chart = getInstanceByDom(chartRef.current);
    chart?.setOption(option, true);
    chart?.resize();
  }, [option]);

  // (Re)bind event handlers.
  useEffect(() => {
    if (!chartRef.current || !onEvents) return;
    const chart = getInstanceByDom(chartRef.current);
    if (!chart) return;
    const entries = Object.entries(onEvents);
    // ECharts' `on`/`off` type the handler as `(...args: unknown[]) => …`; our
    // handlers take the concrete event param, so bridge the two here.
    type RawHandler = (...args: unknown[]) => void;
    for (const [name, handler] of entries) {
      chart.on(name, handler as RawHandler);
    }
    return () => {
      for (const [name, handler] of entries) {
        chart.off(name, handler as RawHandler);
      }
    };
  }, [onEvents]);

  // Loading spinner.
  useEffect(() => {
    if (!chartRef.current) return;
    const chart = getInstanceByDom(chartRef.current);
    if (loading) chart?.showLoading();
    else chart?.hideLoading();
  }, [loading]);

  return <div ref={chartRef} className={cn("w-full", className)} style={{ height }} />;
}

export default function EChart(props: EChartProps) {
  return (
    <ErrorBoundary
      resetKeys={[props.option]}
      fallback={
        <ErrorAlert title='Failed to render chart' message='This chart could not be displayed.' />
      }
    >
      <EChartInner {...props} />
    </ErrorBoundary>
  );
}
