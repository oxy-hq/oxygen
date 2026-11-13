import { useCallback } from "react";

type TimeDimension = { dimension: string; granularity?: string };

export const isTimeDimension = (type: string) => type === "date" || type === "datetime";

export const useTimeDimensionHandlers = (
  timeDimensions: TimeDimension[],
  onAddTimeDimension: (initialValues?: { dimension: string; granularity: string }) => void,
  onUpdateTimeDimension: (index: number, updates: { granularity?: string }) => void,
  onRemoveTimeDimension: (index: number) => void
) => {
  const handleGranularitySelect = useCallback(
    (dimensionFullName: string, granularity: string) => {
      const existingIndex = timeDimensions.findIndex((td) => td.dimension === dimensionFullName);
      if (existingIndex >= 0) {
        if (timeDimensions[existingIndex].granularity === granularity) {
          onRemoveTimeDimension(existingIndex);
        } else {
          onUpdateTimeDimension(existingIndex, { granularity });
        }
      } else {
        onAddTimeDimension({ dimension: dimensionFullName, granularity });
      }
    },
    [timeDimensions, onAddTimeDimension, onUpdateTimeDimension, onRemoveTimeDimension]
  );

  const getSelectedGranularity = useCallback(
    (dimensionFullName: string) =>
      timeDimensions.find((td) => td.dimension === dimensionFullName)?.granularity,
    [timeDimensions]
  );

  return {
    handleGranularitySelect,
    getSelectedGranularity
  };
};
