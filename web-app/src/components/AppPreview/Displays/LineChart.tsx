import type { LineSeriesOption } from "echarts";
import { useCallback } from "react";
import { Echarts } from "@/components/Echarts";
import type { DataContainer, LineChartDisplay } from "@/types/app";
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

export const LineChart = ({
  display,
  data,
  index
}: {
  display: LineChartDisplay;
  data?: DataContainer;
  index?: number;
}) => {
  const buildChartOptions = useCallback(
    async ({ display, connection, fileName, isDarkMode }: ChartBuilderParams<LineChartDisplay>) => {
      const baseOptions = createBaseChartOptions(isDarkMode);
      const xAxis = await getXAxisData(connection, fileName, display.x);
      // Explicit `y_format` wins; otherwise infer from the y column's name and type.
      const yFormat = await resolveValueFormat(connection, fileName, display.y, display.y_format);
      const xyAxisOptions = createXYAxisOptions(xAxis.labels, isDarkMode, yFormat);
      const tooltipFormatter = createAxisTooltipFormatter(yFormat);

      // Configure tooltip to show values on hover
      const tooltipOptions = {
        trigger: "axis" as const,
        axisPointer: {
          type: "line" as const
        },
        ...(tooltipFormatter ? { formatter: tooltipFormatter } : {})
      };

      let series: LineSeriesOption[];

      if (display.series) {
        const allSeries = await getSeriesData(connection, fileName, display.series);
        series = await Promise.all(
          allSeries.map(async ({ name: seriesName, key: seriesKey }): Promise<LineSeriesOption> => {
            const values = await getSeriesValues(
              connection,
              fileName,
              display.x,
              display.y,
              display.series!,
              seriesKey
            );
            const valueMap = new Map(values.map((v) => [v.x, v.y]));
            // Align data with the x axis by each category's full value (its
            // key, not its label), using null for missing values
            const alignedData = xAxis.keys.map((x) => valueMap.get(x) ?? null);
            return {
              name: JSON.stringify(seriesName),
              type: "line",
              data: alignedData,
              showSymbol: false
            };
          })
        );
      } else {
        const values = await getSimpleAggregatedData(connection, fileName, display.x, display.y);
        series = [
          {
            name: display.y,
            type: "line",
            data: values,
            showSymbol: false
          }
        ];
      }

      return {
        ...baseOptions,
        ...xyAxisOptions,
        ...(display.series ? {} : { color: createSingleSeriesPalette() }),
        tooltip: tooltipOptions,
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
      options={chartOptions}
      isLoading={isLoading}
      title={display.title}
      testId='app-line-chart'
      chartIndex={index}
    />
  );
};
