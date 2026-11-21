import { useAutoAnimate } from "@formkit/auto-animate/react";
import type { EChartsOption } from "echarts";
import { ErrorBoundary } from "react-error-boundary";
import { createCategoricalPalette } from "@/components/AppPreview/Displays/hooks/chartOptions";
import { Echarts } from "@/components/Echarts";
import IdeUnavailablePanel from "@/components/ui/IdeUnavailablePanel";
import useChart from "@/hooks/api/useChart";
import { isIdeUnavailableError } from "@/libs/utils/ideHealth";
import useTheme from "@/stores/useTheme";
import type { ChartConfig } from "@/types/chart";
import ChartError from "./Error";
import ChartLoading from "./Loading";

type ChartProps = {
  data: string | undefined;
  isPending: boolean;
  isFetching: boolean;
  error: Error | null;
  refetch: () => void;
};

type Props = {
  chart_src: string;
};

const Chart = (props: ChartProps) => {
  const { theme } = useTheme();
  const { data, isPending, isFetching, error, refetch } = props;

  if (isPending) {
    return <ChartLoading />;
  }

  if (error) {
    // A chart file lives on the ide's local disk; an ide restart is a transient
    // pause, not a broken chart — degrade to the calm "resuming" placeholder.
    if (isIdeUnavailableError(error)) {
      return (
        <IdeUnavailablePanel
          description='This chart needs Oxygen Nexus, which is restarting. It will resume shortly.'
          onRetry={() => void refetch()}
          retrying={isFetching}
        />
      );
    }
    return (
      <ChartError
        title='Failed to load chart'
        description={
          error?.message ?? "There was an error loading the chart data. Please try again."
        }
        refetch={refetch}
      />
    );
  }

  let config: ChartConfig;
  try {
    config = JSON.parse(data || "{}") as ChartConfig;
  } catch {
    return (
      <ChartError
        title='Invalid chart data'
        description='The chart data format is invalid and cannot be displayed.'
        refetch={refetch}
      />
    );
  }

  const isDarkMode = theme === "dark";
  const mutedFg =
    getComputedStyle(document.documentElement).getPropertyValue("--muted-foreground").trim() ||
    (isDarkMode ? "oklch(0.708 0 0)" : "oklch(0.556 0 0)");

  const options: EChartsOption = {
    color: createCategoricalPalette(),
    darkMode: isDarkMode,
    tooltip: {},
    xAxis: config.xAxis
      ? {
          type: (config.xAxis?.type as "category" | "value" | "time" | "log") || "category",
          name: config.xAxis?.name,
          nameTextStyle: {
            color: mutedFg,
            padding: [15, 0, 0, 0]
          },
          nameLocation: "middle",
          data: (config.xAxis?.data || []).map((d: string | number | Date) =>
            d instanceof Date ? d.toISOString() : d
          )
        }
      : undefined,
    yAxis: config.yAxis
      ? {
          type: (config.yAxis?.type as "category" | "value" | "time" | "log") || "category",
          name: config.yAxis?.name,
          nameTextStyle: {
            color: mutedFg
          },
          data: (config.yAxis?.data || []).map((d: string | number | Date) =>
            d instanceof Date ? d.toISOString() : d
          )
        }
      : undefined,
    series: config.series.map((s) => ({
      name: s.name,
      type: s.type,
      data:
        s.data?.map((d) =>
          d && typeof d === "object" && "value" in d ? { name: d.name, value: d.value } : d
        ) || []
    }))
  };

  return <Echarts options={options} isLoading={isPending} title={config.title} />;
};

const ChartContainer = (props: Props) => {
  const [parent] = useAutoAnimate({
    duration: 300
  });
  const encodedPath = encodeURIComponent(props.chart_src);
  const { data, isPending, isFetching, error, refetch } = useChart(encodedPath);

  return (
    <div ref={parent as React.Ref<HTMLDivElement>}>
      <ErrorBoundary
        fallback={
          <ChartError
            title='Chart error'
            description='An unexpected error occurred while rendering the chart.'
            refetch={refetch}
          />
        }
      >
        <Chart
          data={data}
          isPending={isPending}
          isFetching={isFetching}
          error={error}
          refetch={refetch}
        />
      </ErrorBoundary>
    </div>
  );
};

export default ChartContainer;
