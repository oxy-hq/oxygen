import type { EChartsOption } from "echarts";
import { getInstanceByDom, init } from "echarts";
import { useCallback, useEffect, useRef } from "react";
import { ErrorBoundary } from "react-error-boundary";
import { useResizeDetector } from "react-resize-detector";
import theme from "@/components/Echarts/theme.json";
import ErrorAlert from "@/components/ui/ErrorAlert";

interface MiniChartProps {
  options: EChartsOption;
  isLoading: boolean;
  title: string;
}

function MiniChartInner({ options, isLoading }: MiniChartProps) {
  const chartRef = useRef<HTMLDivElement>(null);

  const onResize = useCallback(() => {
    if (chartRef.current) {
      const chart = getInstanceByDom(chartRef.current);
      chart?.resize();
    }
  }, []);

  useResizeDetector({
    targetRef: chartRef,
    onResize
  });

  useEffect(() => {
    if (!chartRef.current) return;
    const chart = init(chartRef.current, theme);

    return () => {
      chart.dispose();
    };
  }, []);

  useEffect(() => {
    if (chartRef.current) {
      const chart = getInstanceByDom(chartRef.current);
      chart?.setOption(options, true);
      chart?.resize();
    }
  }, [options]);

  useEffect(() => {
    if (chartRef.current) {
      const chart = getInstanceByDom(chartRef.current);
      if (isLoading) {
        chart?.showLoading();
      } else {
        chart?.hideLoading();
      }
    }
  }, [isLoading]);

  return (
    <div className='flex flex-col'>
      <div ref={chartRef} className='h-[100px] w-full' />
    </div>
  );
}

export default function MiniChart(props: MiniChartProps) {
  return (
    <ErrorBoundary
      resetKeys={[props.options]}
      fallback={
        <ErrorAlert
          className='m-3'
          title={`Failed to render ${props.title}`}
          message='This metric chart could not be displayed.'
        />
      }
    >
      <MiniChartInner {...props} />
    </ErrorBoundary>
  );
}
