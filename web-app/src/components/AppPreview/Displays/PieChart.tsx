import type { PieSeriesOption } from "echarts";
import { useCallback } from "react";
import { Echarts } from "@/components/Echarts";
import type { DataContainer, PieChartDisplay } from "@/types/app";
import {
  type ChartBuilderParams,
  createPieChartOptions,
  getPieChartData,
  resolveValueFormat,
  useChartBase
} from "./hooks";

export const PieChart = ({
  display,
  data,
  index
}: {
  display: PieChartDisplay;
  data?: DataContainer;
  index?: number;
}) => {
  const buildChartOptions = useCallback(
    async ({ display, connection, fileName, isDarkMode }: ChartBuilderParams<PieChartDisplay>) => {
      // Explicit `value_format` wins; otherwise infer from the value column's
      // name and type.
      const valueFormat = await resolveValueFormat(
        connection,
        fileName,
        display.value,
        display.value_format
      );
      const baseOptions = createPieChartOptions(isDarkMode, valueFormat, true);

      const pieData = await getPieChartData(connection, fileName, display.name, display.value);

      const pieSeries: PieSeriesOption = {
        type: "pie",
        data: pieData,
        emphasis: {
          itemStyle: {
            shadowBlur: 10,
            shadowOffsetX: 0,
            shadowColor: "rgba(0, 0, 0, 0.5)"
          }
        }
      };

      return {
        ...baseOptions,
        series: [pieSeries]
      };
    },
    []
  );

  const { isLoading, chartOptions } = useChartBase({
    display,
    data,
    buildChartOptions
  });

  return (
    <Echarts
      isLoading={isLoading}
      chartIndex={index}
      options={chartOptions}
      title={display.title}
    />
  );
};
