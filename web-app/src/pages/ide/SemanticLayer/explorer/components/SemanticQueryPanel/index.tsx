import { type ReactNode, useEffect, useMemo } from "react";
import SqlView from "@/components/SemanticQueryPanel/SqlView";
import { Tabs, TabsContent } from "@/components/ui/shadcn/tabs";
import { useSemanticExplorerContext } from "../../contexts/SemanticExplorerContext";
import FiltersSection from "./components/FiltersSection";
import ResultsView from "./components/ResultsView";
import SortsSection from "./components/SortsSection";
import TabsHeader from "./components/TabsHeader";
import VariablesSection from "./components/VariablesSection";

export interface Variable {
  key: string;
  value: string;
}

interface SemanticQueryPanelProps {
  extraSectionAboveSorts?: ReactNode;
  showAddVariable?: boolean;
  sqlLoadingIndicator?: ReactNode;
  executeLoadingIndicator?: ReactNode;
}

const SemanticQueryPanel = ({
  extraSectionAboveSorts,
  showAddVariable = true,
  sqlLoadingIndicator,
  executeLoadingIndicator
}: SemanticQueryPanelProps) => {
  const {
    result,
    showSql,
    setShowSql,
    generatedSql,
    sqlError,
    executionError,
    filters,
    orders,
    variables,
    timeDimensions,
    onAddFilter,
    onUpdateFilter,
    onRemoveFilter,
    onAddOrder,
    onUpdateOrder,
    onRemoveOrder,
    onAddVariable,
    onUpdateVariable,
    onRemoveVariable,
    resultFile,
    onExecuteQuery,
    sqlLoading,
    executeLoading,
    availableDimensions,
    availableMeasures,
    selectedDimensions,
    selectedMeasures,
    limit,
    onLimitChange,
    isPreagg,
    executionTime
  } = useSemanticExplorerContext();
  const availableFields = useMemo(() => {
    return [...availableDimensions, ...availableMeasures];
  }, [availableDimensions, availableMeasures]);

  const selectedFields = useMemo(() => {
    const selectedDimensionFields = selectedDimensions
      .map((dim) => availableFields.find((f) => f.fullName === dim))
      .filter((field): field is (typeof availableFields)[number] => field !== undefined);

    const selectedMeasureFields = selectedMeasures
      .map((mes) => availableFields.find((f) => f.fullName === mes))
      .filter((field): field is (typeof availableFields)[number] => field !== undefined);

    const selectedTimeDimensionFields = timeDimensions
      .map((td) => availableFields.find((f) => f.fullName === td.dimension))
      .filter((field): field is (typeof availableFields)[number] => field !== undefined);

    return [...selectedDimensionFields, ...selectedMeasureFields, ...selectedTimeDimensionFields];
  }, [selectedDimensions, selectedMeasures, timeDimensions, availableFields]);

  const canExecuteQuery =
    selectedDimensions.length > 0 || selectedMeasures.length > 0 || timeDimensions.length > 0;

  useEffect(() => {
    if (generatedSql || sqlError) {
      setShowSql(true);
    }
  }, [generatedSql, setShowSql, sqlError]);

  useEffect(() => {
    if (executeLoading || result.length > 0 || resultFile || executionError) {
      setShowSql(false);
    }
  }, [executeLoading, result, resultFile, setShowSql, executionError]);

  return (
    <Tabs
      value={showSql ? "sql" : "results"}
      onValueChange={(value) => setShowSql(value === "sql")}
      className='flex min-h-0 flex-1 flex-col gap-0 overflow-hidden'
    >
      <TabsHeader
        showSql={showSql}
        hasResults={result.length > 0}
        result={result}
        onAddFilter={onAddFilter}
        onAddOrder={onAddOrder}
        onAddVariable={onAddVariable}
        showAddVariable={showAddVariable}
        onExecuteQuery={onExecuteQuery}
        loading={executeLoading}
        canExecuteQuery={canExecuteQuery}
        disabledMessage=''
        hasSelectedFields={selectedFields.length > 0}
        limit={limit}
        onLimitChange={onLimitChange}
      />

      <FiltersSection
        filters={filters}
        availableDimensions={availableDimensions}
        onUpdateFilter={onUpdateFilter}
        onRemoveFilter={onRemoveFilter}
      />

      {extraSectionAboveSorts}

      <SortsSection
        orders={orders}
        availableFields={selectedFields}
        onUpdateOrder={onUpdateOrder}
        onRemoveOrder={onRemoveOrder}
      />

      <VariablesSection
        variables={variables}
        onUpdateVariable={onUpdateVariable}
        onRemoveVariable={onRemoveVariable}
      />

      <div className='min-h-0 flex-1 overflow-hidden'>
        <TabsContent value='sql' className='mt-0 h-full'>
          <SqlView
            generatedSql={generatedSql}
            sqlError={sqlError}
            loading={sqlLoading}
            loadingIndicator={sqlLoadingIndicator}
          />
        </TabsContent>
        <TabsContent value='results' className='mt-0 h-full'>
          <ResultsView
            result={result}
            resultFile={resultFile}
            executionError={executionError}
            loading={executeLoading}
            loadingIndicator={executeLoadingIndicator}
            isPreagg={isPreagg}
            executionTime={executionTime}
          />
        </TabsContent>
      </div>
    </Tabs>
  );
};

export default SemanticQueryPanel;
