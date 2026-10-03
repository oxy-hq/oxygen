import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState
} from "react";
import useCompileLookerQuery from "@/hooks/api/integrations/useCompileLookerQuery";
import useExecuteLookerQuery from "@/hooks/api/integrations/useExecuteLookerQuery";
import useLookerIntegrations from "@/hooks/api/integrations/useLookerIntegrations";
import {
  SemanticExplorerContext,
  type SemanticExplorerContextType
} from "../../contexts/SemanticExplorerContext";
import type { Field, Order } from "../../types";

export type LookerFilter = { id: string; field: string; value: string };

type LookerExplorerProviderProps = {
  children: ReactNode;
  integrationName: string;
  model: string;
  exploreName: string;
};

type LookerExplorerContextType = {
  exploreName: string;
  integrationName: string;
  model: string;
  dimensions: string[];
  measures: string[];
  allFields: string[];
  exploreLoading: boolean;
  exploreError: Error | null;
  lookerFilters: LookerFilter[];
  onUpdateLookerFilter: (index: number, updates: LookerFilter) => void;
  onRemoveLookerFilter: (index: number) => void;
};

const LookerExplorerContext = createContext<LookerExplorerContextType | null>(null);

export const useLookerExplorerContext = () => {
  const ctx = useContext(LookerExplorerContext);
  if (!ctx) throw new Error("useLookerExplorerContext must be used within LookerExplorerProvider");
  return ctx;
};

export const LookerExplorerProvider = ({
  children,
  integrationName,
  model,
  exploreName
}: LookerExplorerProviderProps) => {
  const { data: integrations, isLoading, error } = useLookerIntegrations();
  const { mutate: executeLookerQuery, isPending: isExecuting } = useExecuteLookerQuery();
  const { mutate: compileLookerQuery, isPending: isCompiling } = useCompileLookerQuery();

  const [result] = useState<string[][]>([]);
  const [resultFile, setResultFile] = useState<string | undefined>(undefined);
  const [executionError, setExecutionError] = useState<string | null>(null);
  const [selectedDimensions, setSelectedDimensions] = useState<string[]>([]);
  const [selectedMeasures, setSelectedMeasures] = useState<string[]>([]);
  const [lookerFilters, setLookerFilters] = useState<LookerFilter[]>([]);
  const [orders, setOrders] = useState<Order[]>([]);
  const [showSql, setShowSql] = useState(false);
  const [limit, setLimit] = useState(1000);
  const [generatedSql, setGeneratedSql] = useState("");
  const [sqlError, setSqlError] = useState<string | null>(null);

  const explore = useMemo(() => {
    if (!integrations) return null;
    const integration = integrations.find((i) => i.name === integrationName);
    return integration?.explores.find((e) => e.model === model && e.name === exploreName) ?? null;
  }, [integrations, integrationName, model, exploreName]);

  const dimensions = explore?.dimensions ?? [];
  const measures = explore?.measures ?? [];

  const allFields = useMemo(() => [...dimensions, ...measures], [dimensions, measures]);

  const availableDimensions = useMemo<Field[]>(
    () => dimensions.map((f) => ({ name: f, fullName: f, type: "string" as const })),
    [dimensions]
  );

  const availableMeasures = useMemo<Field[]>(
    () => measures.map((f) => ({ name: f, fullName: f })),
    [measures]
  );

  const removeOrdersForField = useCallback((fieldName: string) => {
    setOrders((prev) => prev.filter((o) => o.field !== fieldName));
  }, []);

  const toggleDimension = useCallback(
    (fullName: string) => {
      setSelectedDimensions((prev) => {
        if (prev.includes(fullName)) {
          removeOrdersForField(fullName);
          return prev.filter((d) => d !== fullName);
        }
        return [...prev, fullName];
      });
    },
    [removeOrdersForField]
  );

  const toggleMeasure = useCallback(
    (fullName: string) => {
      setSelectedMeasures((prev) => {
        if (prev.includes(fullName)) {
          removeOrdersForField(fullName);
          return prev.filter((m) => m !== fullName);
        }
        return [...prev, fullName];
      });
    },
    [removeOrdersForField]
  );

  const addLookerFilter = useCallback(() => {
    if (allFields.length > 0) {
      setLookerFilters((prev) => [
        ...prev,
        { id: crypto.randomUUID(), field: allFields[0], value: "" }
      ]);
    }
  }, [allFields]);

  const onUpdateLookerFilter = useCallback((index: number, updates: LookerFilter) => {
    setLookerFilters((prev) => {
      const next = [...prev];
      next[index] = updates;
      return next;
    });
  }, []);

  const onRemoveLookerFilter = useCallback((index: number) => {
    setLookerFilters((prev) => prev.filter((_, i) => i !== index));
  }, []);

  const addOrder = useCallback(() => {
    const first = selectedDimensions[0] ?? selectedMeasures[0];
    if (first) {
      setOrders((prev) => [...prev, { field: first, direction: "asc" }]);
    }
  }, [selectedDimensions, selectedMeasures]);

  const updateOrder = useCallback((index: number, updates: Order) => {
    setOrders((prev) => {
      const next = [...prev];
      next[index] = updates;
      return next;
    });
  }, []);

  const removeOrder = useCallback((index: number) => {
    setOrders((prev) => prev.filter((_, i) => i !== index));
  }, []);

  const selectedFields = useMemo(
    () => [...selectedDimensions, ...selectedMeasures],
    [selectedDimensions, selectedMeasures]
  );

  const buildFiltersMap = useCallback((): Record<string, string> | undefined => {
    const map: Record<string, string> = {};
    for (const f of lookerFilters) {
      if (f.field && f.value !== "") {
        map[f.field] = f.value;
      }
    }
    return Object.keys(map).length > 0 ? map : undefined;
  }, [lookerFilters]);

  const handleExecuteQuery = useCallback(() => {
    if (selectedFields.length === 0) return;

    executeLookerQuery(
      {
        integration: integrationName,
        model,
        explore: exploreName,
        fields: selectedFields,
        filters: buildFiltersMap(),
        sorts:
          orders.length > 0
            ? orders.map((o) => ({ field: o.field, direction: o.direction }))
            : undefined,
        limit
      },
      {
        onSuccess: (data) => {
          setResultFile(data.file_name);
          setExecutionError(null);
          setShowSql(false);
        },
        onError: (err) => {
          setResultFile(undefined);
          setExecutionError(err.message);
        }
      }
    );
  }, [
    selectedFields,
    buildFiltersMap,
    orders,
    integrationName,
    model,
    exploreName,
    executeLookerQuery,
    limit
  ]);

  useEffect(() => {
    if (selectedFields.length === 0) {
      setGeneratedSql("");
      setSqlError(null);
      return;
    }

    setShowSql(true);
    setGeneratedSql("");
    setSqlError(null);

    compileLookerQuery(
      {
        integration: integrationName,
        model,
        explore: exploreName,
        fields: selectedFields,
        filters: buildFiltersMap(),
        sorts:
          orders.length > 0
            ? orders.map((o) => ({ field: o.field, direction: o.direction }))
            : undefined,
        limit
      },
      {
        onSuccess: (sql) => {
          setGeneratedSql(sql);
          setSqlError(null);
        },
        onError: (err) => {
          setGeneratedSql("");
          setSqlError(err.message);
        }
      }
    );
  }, [
    selectedFields,
    orders,
    limit,
    integrationName,
    model,
    exploreName,
    compileLookerQuery,
    buildFiltersMap
  ]);

  const semanticContextValue = useMemo<SemanticExplorerContextType>(
    () => ({
      dataLoading: isLoading,
      loadingError: error?.message,
      loading: isLoading || isExecuting || isCompiling,
      sqlLoading: isCompiling,
      executeLoading: isExecuting,
      selectedDimensions,
      selectedMeasures,
      toggleDimension,
      toggleMeasure,
      result,
      showSql,
      setShowSql,
      generatedSql,
      sqlError,
      executionError,
      filters: [],
      orders,
      variables: [],
      timeDimensions: [],
      onAddFilter: addLookerFilter,
      // Looker filters are managed via LookerExplorerContext; these are intentional no-ops for the shared SemanticExplorer interface
      onUpdateFilter: () => {},
      onRemoveFilter: () => {},
      onAddOrder: addOrder,
      onUpdateOrder: updateOrder,
      onRemoveOrder: removeOrder,
      onAddVariable: () => {},
      onUpdateVariable: () => {},
      onRemoveVariable: () => {},
      onAddTimeDimension: () => {},
      onUpdateTimeDimension: () => {},
      onRemoveTimeDimension: () => {},
      onExecuteQuery: handleExecuteQuery,
      availableDimensions,
      availableMeasures,
      setGeneratedSql,
      setSqlError,
      canExecuteQuery: selectedFields.length > 0,
      limit,
      onLimitChange: setLimit,
      resultFile,
      setSelectedDimensions: () => {},
      setSelectedMeasures: () => {},
      isPreagg: false,
      executionTime: null
    }),
    [
      isLoading,
      isExecuting,
      error,
      selectedDimensions,
      selectedMeasures,
      toggleDimension,
      toggleMeasure,
      result,
      resultFile,
      showSql,
      executionError,
      orders,
      addLookerFilter,
      addOrder,
      updateOrder,
      removeOrder,
      handleExecuteQuery,
      availableDimensions,
      availableMeasures,
      selectedFields,
      limit,
      generatedSql,
      sqlError,
      isCompiling
    ]
  );

  const lookerContextValue = useMemo<LookerExplorerContextType>(
    () => ({
      exploreName,
      integrationName,
      model,
      dimensions,
      measures,
      allFields,
      exploreLoading: isLoading,
      exploreError: error,
      lookerFilters,
      onUpdateLookerFilter,
      onRemoveLookerFilter
    }),
    [
      exploreName,
      integrationName,
      model,
      dimensions,
      measures,
      allFields,
      isLoading,
      error,
      lookerFilters,
      onUpdateLookerFilter,
      onRemoveLookerFilter
    ]
  );

  return (
    <LookerExplorerContext.Provider value={lookerContextValue}>
      <SemanticExplorerContext.Provider value={semanticContextValue}>
        {children}
      </SemanticExplorerContext.Provider>
    </LookerExplorerContext.Provider>
  );
};
