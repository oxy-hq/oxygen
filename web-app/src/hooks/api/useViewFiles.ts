import { useMemo } from "react";
import useSemanticObjects from "./useSemanticObjects";

export interface ViewFileOption {
  value: string;
  label: string;
  path: string;
  searchText: string;
}

/** The served revision's views, as picker options. */
export default function useViewFiles() {
  const { data, isLoading, error } = useSemanticObjects();

  const viewFiles = useMemo(
    (): ViewFileOption[] =>
      (data?.views ?? []).map((v) => ({
        value: v.name,
        label: v.name,
        path: v.path,
        searchText: `${v.name} ${v.path}`.toLowerCase()
      })),
    [data]
  );

  return { viewFiles, isLoading, error };
}
