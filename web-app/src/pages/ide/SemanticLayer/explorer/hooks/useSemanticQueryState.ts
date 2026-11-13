import { useState } from "react";
import type { TimeDimension } from "@/types/artifact";
import type { Variable } from "../components/SemanticQueryPanel";
import type { Filter, Order } from "../types";

export const useSemanticQueryState = () => {
  const [result, setResult] = useState<string[][]>([]);
  const [resultFile, setResultFile] = useState<string | undefined>(undefined);
  const [isPreagg, setIsPreagg] = useState(false);
  const [selectedDimensions, setSelectedDimensions] = useState<string[]>([]);
  const [selectedMeasures, setSelectedMeasures] = useState<string[]>([]);
  const [filters, setFilters] = useState<Filter[]>([]);
  const [orders, setOrders] = useState<Order[]>([]);
  const [variables, setVariables] = useState<Variable[]>([]);
  const [timeDimensions, setTimeDimensions] = useState<TimeDimension[]>([]);
  const [showSql, setShowSql] = useState(false);
  const [generatedSql, setGeneratedSql] = useState("");
  const [sqlError, setSqlError] = useState<string | null>(null);
  const [executionError, setExecutionError] = useState<string | null>(null);
  const [executionTime, setExecutionTime] = useState<number | null>(null);

  const addFilter = (initialField: string) => {
    setFilters([
      ...filters,
      {
        field: initialField,
        op: "eq",
        value: ""
      }
    ]);
  };

  const updateFilter = (index: number, updates: Filter) => {
    const newFilters = [...filters];
    newFilters[index] = updates;
    setFilters(newFilters);
  };

  const removeFilter = (index: number) => {
    setFilters(filters.filter((_, i) => i !== index));
  };

  const addOrder = (initialField: string) => {
    setOrders([
      ...orders,
      {
        field: initialField,
        direction: "asc"
      }
    ]);
  };

  const updateOrder = (index: number, updates: Order) => {
    const newOrders = [...orders];
    newOrders[index] = updates;
    setOrders(newOrders);
  };

  const removeOrder = (index: number) => {
    setOrders(orders.filter((_, i) => i !== index));
  };

  const addVariable = () => {
    setVariables([...variables, { key: "", value: "" }]);
  };

  const updateVariable = (index: number, updates: Partial<Variable>) => {
    const newVariables = [...variables];
    newVariables[index] = { ...newVariables[index], ...updates };
    setVariables(newVariables);
  };

  const removeVariable = (index: number) => {
    setVariables(variables.filter((_, i) => i !== index));
  };

  const addTimeDimension = (initialValues?: Partial<TimeDimension>) => {
    setTimeDimensions([
      ...timeDimensions,
      {
        dimension: "",
        granularity: "value",
        ...initialValues
      }
    ]);
  };

  const updateTimeDimension = (index: number, updates: Partial<TimeDimension>) => {
    const newTimeDimensions = [...timeDimensions];
    newTimeDimensions[index] = { ...newTimeDimensions[index], ...updates };
    setTimeDimensions(newTimeDimensions);
  };

  const removeTimeDimension = (index: number) => {
    setTimeDimensions(timeDimensions.filter((_, i) => i !== index));
  };

  const removeOrdersForField = (fieldName: string) => {
    setOrders((prevOrders) => prevOrders.filter((order) => order.field !== fieldName));
  };

  const toggleDimension = (fullName: string) => {
    const isRemoving = selectedDimensions.includes(fullName);
    if (isRemoving) {
      removeOrdersForField(fullName);
      setSelectedDimensions((prev) => prev.filter((d) => d !== fullName));
    } else {
      setSelectedDimensions((prev) => [...prev, fullName]);
    }
  };

  const toggleMeasure = (fullName: string) => {
    const isRemoving = selectedMeasures.includes(fullName);
    if (isRemoving) {
      removeOrdersForField(fullName);
      setSelectedMeasures((prev) => prev.filter((m) => m !== fullName));
    } else {
      setSelectedMeasures((prev) => [...prev, fullName]);
    }
  };

  return {
    result,
    setResult,
    resultFile,
    setResultFile,
    isPreagg,
    setIsPreagg,
    selectedDimensions,
    setSelectedDimensions,
    selectedMeasures,
    setSelectedMeasures,
    filters,
    setFilters,
    orders,
    setOrders,
    variables,
    setVariables,
    timeDimensions,
    setTimeDimensions,
    showSql,
    setShowSql,
    generatedSql,
    setGeneratedSql,
    sqlError,
    setSqlError,
    executionError,
    setExecutionError,
    executionTime,
    setExecutionTime,
    addFilter,
    updateFilter,
    removeFilter,
    addOrder,
    updateOrder,
    removeOrder,
    addVariable,
    updateVariable,
    removeVariable,
    addTimeDimension,
    updateTimeDimension,
    removeTimeDimension,
    toggleDimension,
    toggleMeasure
  };
};
