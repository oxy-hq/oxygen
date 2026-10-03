import type { BarSeriesOption } from "echarts";
import { useCallback } from "react";
import { Echarts } from "@/components/Echarts";
import type { BarChartDisplay, DataContainer } from "@/types/app";
import {
  type ChartBuilderParams,
  createAxisTooltipFormatter,
  createBaseChartOptions,
  createSingleSeriesPalette,
  createXYAxisOptions,
  getSeriesData,
  getSeriesValues,
  getSimpleAggregatedData,
  getXAxisData,
  resolveValueFormat,
  useChartBase
} from "./hooks";

export const BarChart = ({
  display,
  data,
  index
}: {
  display: BarChartDisplay;
  data?: DataContainer;
  index?: number;
}) => {
  const buildChartOptions = useCallback(
    async ({ display, connection, fileName, isDarkMode }: ChartBuilderParams<BarChartDisplay>) => {
      const baseOptions = createBaseChartOptions(isDarkMode);
      const xData = await getXAxisData(connection, fileName, display.x);
      // Explicit `y_format` wins; otherwise infer currency from the y
      // column's name and type so dashboards built before `y_format` existed
      // still render monetary columns as dollars.
      const yFormat = await resolveValueFormat(connection, fileName, display.y, display.y_format);
      const xyAxisOptions = createXYAxisOptions(xData, isDarkMode, yFormat);
      const tooltipFormatter = createAxisTooltipFormatter(yFormat);

      let series: BarSeriesOption[];

      if (display.series) {
        const seriesNames = await getSeriesData(connection, fileName, display.series);
        series = await Promise.all(
          seriesNames.map(async (seriesName): Promise<BarSeriesOption> => {
            const values = await getSeriesValues(
              connection,
              fileName,
              display.x,
              display.y,
              display.series!,
              seriesName
            );
            const valueMap = new Map(values.map((v) => [v.x, v.y]));
            // Align data with xData axis, using null for missing values
            const alignedData = xData.map((x) => valueMap.get(x) ?? null);
            return {
              name: String(seriesName),
              type: "bar",
              stack: "total",
              data: alignedData
            };
          })
        );
      } else {
        const values = await getSimpleAggregatedData(connection, fileName, display.x, display.y);
        series = [
          {
            name: display.y,
            type: "bar",
            data: values
          }
        ];
      }

      return {
        ...baseOptions,
        ...xyAxisOptions,
        ...(display.series ? {} : { color: createSingleSeriesPalette() }),
        ...(tooltipFormatter
          ? {
              tooltip: {
                trigger: "axis" as const,
                axisPointer: { type: "shadow" as const },
                formatter: tooltipFormatter
              }
            }
          : {}),
        series
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
      options={chartOptions}
      title={display.title}
      testId='app-bar-chart'
      chartIndex={index}
    />
  );
};
