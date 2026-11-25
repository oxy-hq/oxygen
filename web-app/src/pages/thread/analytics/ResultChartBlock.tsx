import { useMemo } from "react";
import { createCategoricalPalette } from "@/components/AppPreview/Displays/hooks/chartOptions";
import { Echarts } from "@/components/Echarts";
import type { ResultChartData } from "@/services/api/analytics";
import useTheme from "@/stores/useTheme";
import { resultChartOption } from "./resultChartOption";

/**
 * A chart the react agent drew, from the points its row carries: the result's rows as
 * the query returned them. It needs no fetch, so it still draws after the stored
 * result expires.
 */
export function ResultChartBlock({ chart, index }: { chart: ResultChartData; index: number }) {
  const { theme } = useTheme();
  // ECharts paints to canvas and cannot read CSS variables: rebuild on a theme change.
  const options = useMemo(
    () => ({ ...resultChartOption(chart, createCategoricalPalette()), darkMode: theme === "dark" }),
    [chart, theme]
  );
  return (
    <Echarts
      options={options}
      isLoading={false}
      title={chart.title}
      testId='react-result-chart'
      chartIndex={index}
      height='360px'
    />
  );
}
