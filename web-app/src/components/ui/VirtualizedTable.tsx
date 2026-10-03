import { ChevronDown, ChevronsUpDown, ChevronUp, Download } from "lucide-react";
import Papa from "papaparse";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import {
  cellText,
  getArrowExportText,
  getArrowFieldType,
  getArrowResultCell
} from "@/components/AppPreview/Displays/utils";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Button } from "@/components/ui/shadcn/button";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { getDuckDB, registerAuthenticatedParquetFile } from "@/libs/duckdb";

interface VirtualizedTableProps {
  filePath: string;
  pageSize?: number;
  maxHeight?: string;
}

type SortDirection = "asc" | "desc" | null;

interface SortConfig {
  column: string | null;
  direction: SortDirection;
}

interface DataCellProps {
  cell: string;
  rowIdx: number;
  cellIdx: number;
  isSelected: boolean;
  onClick: () => void;
}

const DataCell = memo(
  ({ cell, isSelected, onClick }: DataCellProps) => (
    <div
      className={`flex h-7 cursor-pointer items-center overflow-hidden border-r px-3 py-1 last:border-r-0 ${
        isSelected ? "bg-primary/20 ring-2 ring-primary ring-inset" : "hover:bg-muted/50"
      }`}
      title={cell}
      onClick={onClick}
    >
      <span className='truncate'>{cell}</span>
    </div>
  ),
  (prevProps, nextProps) =>
    prevProps.cell === nextProps.cell && prevProps.isSelected === nextProps.isSelected
);

DataCell.displayName = "DataCell";

export const VirtualizedTable = ({
  filePath,
  pageSize = 1000,
  maxHeight = undefined
}: VirtualizedTableProps) => {
  const { project, branchName } = useCurrentProjectBranch();
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [columns, setColumns] = useState<string[]>([]);
  const [data, setData] = useState<unknown[][]>([]);
  const [totalRows, setTotalRows] = useState(0);
  const [currentPage, setCurrentPage] = useState(0);
  const [sortConfig, setSortConfig] = useState<SortConfig>({
    column: null,
    direction: null
  });
  const [tableName, setTableName] = useState<string>("");
  const tableNameRef = useRef<string>("");

  // Use refs for columns and schema to avoid triggering refetches
  const columnsRef = useRef<string[]>([]);
  const schemaRef = useRef<unknown>(null);
  // Track which filePath is currently registered to detect changes
  const registeredFilePathRef = useRef<string>("");
  const [selectedCell, setSelectedCell] = useState<{
    row: number;
    col: number;
  } | null>(null);

  // Track custom column widths (null means use default)
  const [customColumnWidths, setCustomColumnWidths] = useState<Map<number, number>>(new Map());

  const [resizingColumn, setResizingColumn] = useState<{
    index: number;
    startX: number;
    startWidth: number;
  } | null>(null);

  // Compute column widths: use custom width if available, otherwise calculate based on column name
  const columnWidths = useMemo(() => {
    if (columns.length === 0) return [];
    const numCols = columns.length;
    return Array.from({ length: numCols }, (_, i) => {
      if (customColumnWidths.has(i)) {
        return customColumnWidths.get(i)!;
      }
      // Calculate width based on column name length (roughly 8px per character + padding)
      const columnName = columns[i];
      const calculatedWidth = Math.max(100, columnName.length * 8 + 60);
      return calculatedWidth;
    });
  }, [columns, customColumnWidths]);

  const loadData = useCallback(
    async (page: number, sort: SortConfig) => {
      try {
        const db = await getDuckDB();
        const conn = await db.connect();
        // Closed again whether the queries succeed or fail: a connection is opened
        // for every page and every sort.
        try {
          // Use a local variable to track the table name for this execution
          let tableToQuery = tableNameRef.current;

          // Register the file if not already registered OR if filePath has changed
          const needsRegistration = registeredFilePathRef.current !== filePath;

          if (needsRegistration) {
            const registeredName = await registerAuthenticatedParquetFile(
              filePath,
              project.id,
              branchName
            );
            // Set ref before state so that if this effect is cancelled and re-runs,
            // needsRegistration is already false and we don't register again.
            tableNameRef.current = registeredName;
            registeredFilePathRef.current = filePath;
            setTableName(registeredName);
            tableToQuery = registeredName;

            const countResult = await conn.query(
              `SELECT COUNT(*) as count FROM "${registeredName}"`
            );
            setTotalRows(Number(countResult.toArray()[0].count));

            // Get columns
            const schemaResult = await conn.query(`SELECT * FROM "${registeredName}" LIMIT 0`);
            const cols = schemaResult.schema.fields.map((f) => f.name);
            columnsRef.current = cols;
            schemaRef.current = schemaResult.schema;
            setColumns(cols);
          }

          // Build query with sorting
          const offset = page * pageSize;
          let query = `SELECT * FROM "${tableToQuery}"`;

          if (sort.column && sort.direction) {
            query += ` ORDER BY "${sort.column}" ${sort.direction.toUpperCase()}`;
          }

          // Add pagination
          query += ` LIMIT ${pageSize} OFFSET ${offset}`;

          const result = await conn.query(query);
          const rows = result.toArray();

          // Convert to array format for rendering. This table reports the data,
          // so a number is read as the value it is, not rounded for display.
          const formattedData = rows.map((row) =>
            columnsRef.current.map((col) => {
              const value = (row as Record<string, unknown>)[col];
              if (schemaRef.current) {
                const fieldType = getArrowFieldType(col, result.schema);
                return fieldType ? getArrowResultCell(value, fieldType) : value;
              }
              return value;
            })
          );

          setData(formattedData);
        } finally {
          await conn.close();
        }
      } catch (err) {
        console.error("Error loading data:", err);
        throw err;
      }
    },
    [filePath, project.id, branchName, pageSize]
  );

  useEffect(() => {
    let cancelled = false;

    const fetchData = async () => {
      setIsLoading(true);
      setError(null);

      try {
        await loadData(currentPage, sortConfig);
        if (!cancelled) {
          setIsLoading(false);
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load data");
          setIsLoading(false);
        }
      }
    };

    void fetchData();

    return () => {
      cancelled = true;
    };
  }, [currentPage, sortConfig, loadData]);

  // Handle keyboard shortcuts for copy
  useEffect(() => {
    if (data.length === 0) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "c" && selectedCell) {
        const value = data[selectedCell.row - 1]?.[selectedCell.col];
        if (value !== undefined) {
          navigator.clipboard.writeText(cellText(value)).catch((err) => {
            console.error("Failed to copy:", err);
          });
        }
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [selectedCell, data]);

  useEffect(() => {
    if (!resizingColumn) return;

    let rafId: number | null = null;
    let latestWidth: number | null = null;

    const updateColumnWidth = () => {
      if (latestWidth !== null) {
        setCustomColumnWidths((prev) => {
          const updated = new Map(prev);
          updated.set(resizingColumn.index, latestWidth!);
          return updated;
        });
      }
      rafId = null;
    };

    const handleMouseMove = (e: MouseEvent) => {
      const deltaX = e.clientX - resizingColumn.startX;
      const newWidth = Math.max(50, resizingColumn.startWidth + deltaX);
      latestWidth = newWidth;

      // Throttle updates using requestAnimationFrame
      if (rafId === null) {
        rafId = requestAnimationFrame(updateColumnWidth);
      }
    };

    const handleMouseUp = () => {
      if (rafId !== null) {
        cancelAnimationFrame(rafId);
      }
      setResizingColumn(null);
    };

    window.addEventListener("mousemove", handleMouseMove);
    window.addEventListener("mouseup", handleMouseUp);

    return () => {
      if (rafId !== null) {
        cancelAnimationFrame(rafId);
      }
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("mouseup", handleMouseUp);
    };
  }, [resizingColumn]);

  const handleSort = (column: string) => {
    setSortConfig((prev) => {
      if (prev.column === column) {
        // Cycle through: asc -> desc -> null
        if (prev.direction === "asc") {
          return { column, direction: "desc" };
        } else if (prev.direction === "desc") {
          return { column: null, direction: null };
        }
      }
      return { column, direction: "asc" };
    });
    setCurrentPage(0); // Reset to first page when sorting
  };

  const handleDownloadCsv = async () => {
    try {
      const db = await getDuckDB();
      const conn = await db.connect();
      // Closed again whether the export succeeds or fails.
      try {
        // Get all data (up to a reasonable limit)
        let query = `SELECT * FROM "${tableName}"`;
        if (sortConfig.column && sortConfig.direction) {
          query += ` ORDER BY "${sortConfig.column}" ${sortConfig.direction.toUpperCase()}`;
        }

        const result = await conn.query(query);
        const rows = result.toArray();

        // Convert to CSV format. A raw cell is not its value: a decimal is an
        // unscaled integer and a date an epoch, so each is read by its column type.
        const columnTypes = columns.map((col) => getArrowFieldType(col, result.schema));
        const csvData = [
          columns,
          ...rows.map((row) =>
            columns.map((col, colIdx) => {
              const value = (row as Record<string, unknown>)[col];
              return getArrowExportText(value, columnTypes[colIdx]);
            })
          )
        ];

        const csvContent = Papa.unparse(csvData);
        const blob = new Blob([csvContent], { type: "text/csv;charset=utf-8;" });
        const url = URL.createObjectURL(blob);
        const a = document.createElement("a");
        a.href = url;
        a.download = "query_result.csv";
        a.click();
        URL.revokeObjectURL(url);
      } finally {
        await conn.close();
      }
    } catch (err) {
      console.error("Error downloading CSV:", err);
      toast.error("Failed to download CSV");
    }
  };

  const handleResizeStart = (colIdx: number, e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setResizingColumn({
      index: colIdx,
      startX: e.clientX,
      startWidth: columnWidths[colIdx] || 150
    });
  };

  const totalPages = Math.ceil(totalRows / pageSize);
  const numColumns = columns.length;
  const columnWidthsString = columnWidths.map((w: number) => `${w}px`).join(" ");
  const gridTemplateColumns =
    columnWidths.length > 0
      ? `60px ${columnWidthsString}`
      : `60px repeat(${numColumns}, minmax(150px, 1fr))`;

  if (error) {
    return <ErrorAlert message={error} />;
  }

  return (
    <div className='flex h-full flex-1 flex-col'>
      {/* Table */}
      <div className='h-full min-h-0 overflow-auto font-mono text-xs' style={{ maxHeight }}>
        {isLoading ? (
          <div className='flex items-center justify-center p-8'>
            <span className='text-muted-foreground'>Loading...</span>
          </div>
        ) : (
          <div className='flex min-w-fit flex-col'>
            {/* Fixed Header */}
            <div
              className='sticky top-0 z-10 grid flex-shrink-0 border-b bg-muted'
              style={{ gridTemplateColumns }}
            >
              {/* Row number header */}
              <div className='flex h-8 items-center justify-center border-r bg-muted/80 px-3 font-semibold uppercase' />

              {columns.map((col, idx) => {
                const isSorted = sortConfig.column === col;
                let sortIcon: React.ReactNode;

                if (isSorted && sortConfig.direction === "asc") {
                  sortIcon = <ChevronUp className='h-4 w-4' />;
                } else if (isSorted && sortConfig.direction === "desc") {
                  sortIcon = <ChevronDown className='h-4 w-4' />;
                } else {
                  sortIcon = <ChevronsUpDown className='h-4 w-4 opacity-30' />;
                }

                return (
                  <div
                    key={col}
                    className={`relative flex h-8 cursor-pointer items-center overflow-hidden border-r px-3 font-semibold uppercase last:border-r-0 ${
                      selectedCell?.row === 0 && selectedCell?.col === idx
                        ? "bg-primary/20 ring-2 ring-primary ring-inset"
                        : "hover:bg-muted-foreground/10"
                    }`}
                    onClick={() => {
                      setSelectedCell({ row: 0, col: idx });
                      handleSort(col);
                    }}
                    title={col}
                  >
                    <span className='flex items-center gap-2 truncate'>
                      {col}
                      {sortIcon}
                    </span>
                    <div
                      className='absolute top-0 right-0 bottom-0 w-1 cursor-col-resize hover:bg-primary/50 active:bg-primary'
                      onMouseDown={(e) => handleResizeStart(idx, e)}
                    />
                  </div>
                );
              })}
            </div>

            {/* Scrollable Body */}
            <div className='flex flex-col'>
              {data.map((row, rowIdx) => (
                <div key={rowIdx} className='grid border-b' style={{ gridTemplateColumns }}>
                  {/* Row number */}
                  <div className='flex h-7 items-center justify-center border-r bg-muted/30 px-3 py-1 text-muted-foreground'>
                    {currentPage * pageSize + rowIdx + 1}
                  </div>

                  {row.map((cell, cellIdx) => (
                    <DataCell
                      key={cellIdx}
                      cell={cellText(cell)}
                      rowIdx={rowIdx}
                      cellIdx={cellIdx}
                      isSelected={selectedCell?.row === rowIdx + 1 && selectedCell?.col === cellIdx}
                      onClick={() => setSelectedCell({ row: rowIdx + 1, col: cellIdx })}
                    />
                  ))}
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* Pagination */}
      <div className='flex shrink-0 items-center justify-between border-t p-4'>
        <div className='text-muted-foreground text-sm'>
          Page {currentPage + 1} of {totalPages}
          {" · "}
          Showing {currentPage * pageSize + 1} - {Math.min((currentPage + 1) * pageSize, totalRows)}{" "}
          of {totalRows}
        </div>
        <div className='flex gap-2'>
          <Button
            variant='outline'
            size='sm'
            onClick={handleDownloadCsv}
            disabled={isLoading || !tableName}
          >
            <Download />
            CSV
          </Button>
          <Button
            variant='outline'
            size='sm'
            onClick={() => setCurrentPage(0)}
            disabled={currentPage === 0 || isLoading}
          >
            First
          </Button>
          <Button
            variant='outline'
            size='sm'
            onClick={() => setCurrentPage((p) => Math.max(0, p - 1))}
            disabled={currentPage === 0 || isLoading}
          >
            Previous
          </Button>
          <Button
            variant='outline'
            size='sm'
            onClick={() => setCurrentPage((p) => Math.min(totalPages - 1, p + 1))}
            disabled={currentPage >= totalPages - 1 || isLoading}
          >
            Next
          </Button>
          <Button
            variant='outline'
            size='sm'
            onClick={() => setCurrentPage(totalPages - 1)}
            disabled={currentPage >= totalPages - 1 || isLoading}
          >
            Last
          </Button>
        </div>
      </div>
    </div>
  );
};
