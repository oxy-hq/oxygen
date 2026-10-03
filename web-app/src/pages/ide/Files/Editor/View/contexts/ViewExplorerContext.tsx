import { createContext, type ReactNode, useContext, useMemo } from "react";
import { useViewDetails } from "@/hooks/api/useSemanticQuery";
import {
  SemanticExplorerProvider,
  useSemanticExplorerContext
} from "../../contexts/SemanticExplorerContext";
import { useEditorContext } from "../../contexts/useEditorContext";
import type { ViewData } from "../../types";

type ViewExplorerProviderProps = {
  children: ReactNode;
  /** When set, drives the explorer directly instead of the editor context. */
  pathb64?: string;
};

type ViewExplorerContextType = {
  viewData: ViewData | null;
  viewError: Error | null;
  viewLoading: boolean;
};

const ViewExplorerContext = createContext<ViewExplorerContextType | null>(null);

const ViewExplorerProviderInner = ({
  children,
  pathb64
}: {
  children: ReactNode;
  pathb64: string;
}) => {
  const { data: viewDetails, isLoading: viewLoading, error: viewError } = useViewDetails(pathb64);

  const viewData = useMemo<ViewData | null>(() => {
    if (!viewDetails) return null;
    return {
      name: viewDetails.name,
      description: viewDetails.description,
      datasource: viewDetails.datasource || "",
      table: viewDetails.table || "",
      dimensions: viewDetails.dimensions || [],
      measures: viewDetails.measures || []
    };
  }, [viewDetails]);

  const availableDimensions = useMemo(() => {
    if (!viewData) return [];
    return viewData.dimensions.map((d) => ({
      name: d.name,
      fullName: `${viewData.name}.${d.name}`,
      type: d.type as "string" | "number" | "date" | "datetime" | "boolean"
    }));
  }, [viewData]);

  const availableMeasures = useMemo(() => {
    if (!viewData) return [];
    return viewData.measures.map((m) => ({
      name: m.name,
      fullName: `${viewData.name}.${m.name}`
    }));
  }, [viewData]);

  const canExecuteQuery = useMemo(() => {
    return !!viewData;
  }, [viewData]);

  return (
    <ViewExplorerContext.Provider value={{ viewData, viewError, viewLoading }}>
      <SemanticExplorerProvider
        dataLoading={viewLoading}
        loadingError={viewError?.message}
        availableDimensions={availableDimensions}
        canExecuteQuery={canExecuteQuery}
        availableMeasures={availableMeasures}
      >
        {children}
      </SemanticExplorerProvider>
    </ViewExplorerContext.Provider>
  );
};

const ViewExplorerProviderFromEditor = ({ children }: { children: ReactNode }) => {
  const { pathb64 } = useEditorContext();
  return (
    <ViewExplorerProviderInner key={pathb64} pathb64={pathb64}>
      {children}
    </ViewExplorerProviderInner>
  );
};

export const ViewExplorerProvider = ({ children, pathb64 }: ViewExplorerProviderProps) => {
  if (pathb64 !== undefined) {
    return (
      <ViewExplorerProviderInner key={pathb64} pathb64={pathb64}>
        {children}
      </ViewExplorerProviderInner>
    );
  }
  return <ViewExplorerProviderFromEditor>{children}</ViewExplorerProviderFromEditor>;
};

export const useViewExplorerContext = () => {
  const semanticContext = useSemanticExplorerContext();
  const viewContext = useContext(ViewExplorerContext);

  if (!viewContext) {
    throw new Error("useViewExplorerContext must be used within ViewExplorerProvider");
  }

  return {
    ...semanticContext,
    ...viewContext
  };
};
