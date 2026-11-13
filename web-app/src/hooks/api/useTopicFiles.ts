import { useMemo } from "react";
import useSemanticObjects from "./useSemanticObjects";

export interface TopicFileOption {
  value: string; // Topic name: "foo" (what gets stored in the form)
  label: string; // Display name: "foo"
  path: string; // Full path: "semantics/topics/foo.topic.yml" (for API calls)
  searchText: string; // For filtering: "foo semantics/topics/foo.topic.yml"
}

/** The served revision's topics, as picker options. */
export default function useTopicFiles() {
  const { data, isLoading, error } = useSemanticObjects();

  const topicFiles = useMemo(
    (): TopicFileOption[] =>
      (data?.topics ?? []).map((t) => ({
        value: t.name,
        label: t.name,
        path: t.path,
        searchText: `${t.name} ${t.path}`.toLowerCase()
      })),
    [data]
  );

  const getPathByTopicName = (topicName: string): string | undefined => {
    return topicFiles.find((t) => t.value === topicName)?.path;
  };

  return {
    topicFiles,
    getPathByTopicName,
    isLoading,
    error
  };
}
