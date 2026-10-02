import { createContext, type ReactNode, useContext, useMemo } from "react";
import { useTopicDetails } from "@/hooks/api/useSemanticQuery";
import {
  SemanticExplorerProvider,
  useSemanticExplorerContext
} from "../../contexts/SemanticExplorerContext";
import { useEditorContext } from "../../contexts/useEditorContext";
import type { TopicData, ViewWithData } from "../../types";

type TopicExplorerProviderProps = {
  children: ReactNode;
  /** When set, drives the explorer directly instead of the editor context. */
  pathb64?: string;
};

type TopicExplorerContextType = {
  topicData: TopicData | null;
  viewsWithData: ViewWithData[];
  topicLoading: boolean;
  loadingTopicError: string | undefined;
  refetchTopicDetails: () => void;
};

const TopicExplorerContext = createContext<TopicExplorerContextType | null>(null);

const TopicExplorerProviderInner = ({
  children,
  pathb64
}: {
  children: ReactNode;
  pathb64: string;
}) => {
  const {
    data: topicDetails,
    isLoading: topicLoading,
    error: loadingTopicError,
    refetch: refetchTopicDetails
  } = useTopicDetails(pathb64);

  const viewsWithData = useMemo<ViewWithData[]>(() => {
    if (!topicDetails?.views) return [];
    return topicDetails.views.map((view) => ({
      viewName: view.view_name,
      name: view.name,
      description: view.description,
      datasource: view.datasource || "",
      table: view.table || "",
      dimensions: view.dimensions || [],
      measures: view.measures || []
    }));
  }, [topicDetails]);

  const availableDimensions = useMemo(() => {
    return viewsWithData.flatMap((view) =>
      view.dimensions.map((dim) => ({
        name: `${view.viewName}.${dim.name}`,
        fullName: `${view.viewName}.${dim.name}`,
        type: dim.type as "string" | "number" | "date" | "datetime" | "boolean"
      }))
    );
  }, [viewsWithData]);

  const availableMeasures = useMemo(() => {
    return viewsWithData.flatMap((view) =>
      view.measures.map((measure) => ({
        name: `${view.viewName}.${measure.name}`,
        fullName: `${view.viewName}.${measure.name}`
      }))
    );
  }, [viewsWithData]);

  const canExecuteQuery = useMemo(() => {
    return viewsWithData.length > 0;
  }, [viewsWithData]);

  const topicData = useMemo<TopicData | null>(() => {
    if (!topicDetails?.topic) return null;
    return {
      name: topicDetails.topic.name,
      description: topicDetails.topic.description,
      views: topicDetails.topic.views || [],
      base_view: topicDetails.topic.base_view
    };
  }, [topicDetails]);

  const topicContextValue = useMemo<TopicExplorerContextType>(
    () => ({
      topicData,
      viewsWithData,
      topicLoading,
      loadingTopicError: loadingTopicError?.message,
      // TanStack's `refetch` resolves with the error rather than rejecting; a failed
      // reload surfaces as `loadingTopicError`.
      refetchTopicDetails: () => {
        void refetchTopicDetails();
      }
    }),
    [topicData, viewsWithData, topicLoading, loadingTopicError, refetchTopicDetails]
  );

  return (
    <TopicExplorerContext.Provider value={topicContextValue}>
      <SemanticExplorerProvider
        topic={topicData?.name}
        dataLoading={topicLoading}
        loadingError={loadingTopicError?.message}
        availableDimensions={availableDimensions}
        availableMeasures={availableMeasures}
        canExecuteQuery={canExecuteQuery}
      >
        {children}
      </SemanticExplorerProvider>
    </TopicExplorerContext.Provider>
  );
};

const TopicExplorerProviderFromEditor = ({ children }: { children: ReactNode }) => {
  const { pathb64 } = useEditorContext();
  return (
    <TopicExplorerProviderInner key={pathb64} pathb64={pathb64}>
      {children}
    </TopicExplorerProviderInner>
  );
};

export const TopicExplorerProvider = ({ children, pathb64 }: TopicExplorerProviderProps) => {
  if (pathb64 !== undefined) {
    return (
      <TopicExplorerProviderInner key={pathb64} pathb64={pathb64}>
        {children}
      </TopicExplorerProviderInner>
    );
  }
  return <TopicExplorerProviderFromEditor>{children}</TopicExplorerProviderFromEditor>;
};

export const useTopicExplorerContext = () => {
  const semanticContext = useSemanticExplorerContext();
  const topicContext = useContext(TopicExplorerContext);

  if (!topicContext) {
    throw new Error("useTopicExplorerContext must be used within TopicExplorerProvider");
  }

  return {
    ...semanticContext,
    ...topicContext
  };
};
