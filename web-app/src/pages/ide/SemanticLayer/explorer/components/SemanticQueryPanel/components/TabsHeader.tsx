import { ArrowUpDown, Download, Filter, Plus, Variable } from "lucide-react";
import Papa from "papaparse";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { handleDownloadFile } from "@/libs/utils/string";
import HeaderActions from "../HeaderActions";
import LimitInput from "./LimitInput";

const COLLAPSE_THRESHOLD = 500;

interface TabsHeaderProps {
  showSql: boolean;
  hasResults: boolean;
  result: string[][];
  onAddFilter: () => void;
  onAddOrder: () => void;
  onAddVariable: () => void;
  showAddVariable?: boolean;
  onExecuteQuery: () => void;
  loading: boolean;
  canExecuteQuery: boolean;
  disabledMessage?: string;
  hasSelectedFields: boolean;
  limit?: number;
  onLimitChange?: (limit: number) => void;
}

const TabsHeader = ({
  showSql,
  hasResults,
  result,
  onAddFilter,
  onAddOrder,
  onAddVariable,
  showAddVariable = true,
  onExecuteQuery,
  loading,
  canExecuteQuery,
  disabledMessage,
  hasSelectedFields,
  limit,
  onLimitChange
}: TabsHeaderProps) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const [isCollapsed, setIsCollapsed] = useState(false);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        setIsCollapsed(entry.contentRect.width < COLLAPSE_THRESHOLD);
      }
    });

    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  const handleDownloadCsv = () => {
    const csvContent = Papa.unparse(result, {
      delimiter: ",",
      header: true,
      skipEmptyLines: true
    });
    const blob = new Blob([csvContent], {
      type: "text/csv;charset=utf-8;"
    });
    handleDownloadFile(blob, "query_results.csv");
  };

  return (
    <div
      ref={containerRef}
      className='scrollbar-none flex items-center justify-between gap-4 overflow-x-auto border-b px-4 py-2'
    >
      <TabsList className='shrink-0'>
        <TabsTrigger value='results'>Results</TabsTrigger>
        <TabsTrigger value='sql'>SQL</TabsTrigger>
      </TabsList>
      <div className='flex shrink-0 items-center gap-2'>
        {!showSql && hasResults && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                size='sm'
                variant='ghost'
                onClick={handleDownloadCsv}
                className='h-7 w-7 shrink-0 p-0'
              >
                <Download className='h-4 w-4' />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Download results as CSV</TooltipContent>
          </Tooltip>
        )}
        {isCollapsed ? (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size='sm' variant='outline' className='h-7 shrink-0'>
                <Plus className='mr-1 h-3 w-3' />
                Add
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align='end'>
              <DropdownMenuItem onClick={onAddFilter}>
                <Filter className='h-4 w-4' />
                Add Filter
              </DropdownMenuItem>
              <DropdownMenuItem onClick={onAddOrder} disabled={!hasSelectedFields}>
                <ArrowUpDown className='h-4 w-4' />
                Add Sort
              </DropdownMenuItem>
              {showAddVariable && (
                <DropdownMenuItem onClick={onAddVariable}>
                  <Variable className='h-4 w-4' />
                  Add Variable
                </DropdownMenuItem>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        ) : (
          <>
            <Button size='sm' variant='outline' onClick={onAddFilter} className='h-7 shrink-0'>
              <Plus className='mr-1 h-3 w-3' />
              Add Filter
            </Button>
            <Button
              size='sm'
              variant='outline'
              onClick={onAddOrder}
              className='h-7 shrink-0'
              disabled={!hasSelectedFields}
            >
              <Plus className='mr-1 h-3 w-3' />
              Add Sort
            </Button>
            {showAddVariable && (
              <Button size='sm' variant='outline' onClick={onAddVariable} className='h-7 shrink-0'>
                <Plus className='mr-1 h-3 w-3' />
                Add Variable
              </Button>
            )}
          </>
        )}
        {limit !== undefined && onLimitChange && (
          <LimitInput limit={limit} onLimitChange={onLimitChange} />
        )}
        <div className='shrink-0'>
          <HeaderActions
            onExecuteQuery={onExecuteQuery}
            loading={loading}
            disabled={!canExecuteQuery}
            disabledMessage={disabledMessage}
          />
        </div>
      </div>
    </div>
  );
};

export default TabsHeader;
