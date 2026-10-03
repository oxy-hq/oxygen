import type React from "react";
import { useEffect, useState } from "react";
import ErrorAlert from "@/components/ui/ErrorAlert";
import {
  Table as DataTable,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { getDuckDB } from "@/libs/duckdb";
import type { DataContainer, TableData, TableDisplay } from "@/types/app";
import {
  cellText,
  formatValue,
  getArrowFieldType,
  getArrowValueWithType,
  getData,
  inferColumnFormat,
  registerFromTableData
} from "./utils";

const load_table = async (
  tableData: { file_path: string; json?: string | null },
  projectId: string,
  branchName: string
) => {
  const db = await getDuckDB();
  const conn = await db.connect();
  try {
    const file_name = await registerFromTableData(tableData, projectId, branchName);
    return await conn.query(`select * from "${file_name}"`);
  } finally {
    await conn.close();
  }
};

export const DataTableBlock = ({
  display,
  data
}: {
  display: TableDisplay;
  data?: DataContainer;
}) => {
  const [isLoading, setIsLoading] = useState(true);
  const { project, branchName } = useCurrentProjectBranch();
  const [table, setTable] = useState<Awaited<ReturnType<typeof load_table>> | null>(null);
  // A load that failed is not a result with no rows: it is reported as itself.
  const [loadError, setLoadError] = useState<string | null>(null);

  const dataAvailable = data && display.data;

  useEffect(() => {
    // A load that a newer one has replaced must not report over it.
    let cancelled = false;
    setIsLoading(true);
    setLoadError(null);
    // Cannot reject: the only await is inside the try/catch below.
    void (async () => {
      if (!dataAvailable) {
        setTable(null);
        setIsLoading(false);
        return;
      }
      const value = getData(data, display.data) as TableData | null;
      if (!value) {
        setTable(null);
        setIsLoading(false);
        return;
      }
      // Empty JSON result → show "No data found" without hitting DuckDB.
      if (typeof value.json === "string" && value.json.trim() === "[]") {
        setTable(null);
        setIsLoading(false);
        return;
      }

      try {
        const table = await load_table(value, project.id, branchName);
        if (!cancelled) setTable(table);
      } catch (error) {
        console.error("Failed to load table data:", error);
        if (!cancelled) {
          setTable(null);
          setLoadError(error instanceof Error ? error.message : "Unknown error");
        }
      } finally {
        if (!cancelled) setIsLoading(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [branchName, data, dataAvailable, display.data, project.id]);

  if (isLoading)
    return <div className='flex h-full w-full items-center justify-center'>Loading...</div>;

  let tableContent: React.ReactNode;
  if (loadError !== null) {
    tableContent = <ErrorAlert title='Failed to load this table' message={loadError} />;
  } else if (!table) {
    tableContent = <div className='p-2 text-center text-muted-foreground'>No data found</div>;
  } else {
    tableContent = (
      <DataTable className='border'>
        <TableHeader>
          <TableRow>
            {table.schema.fields.map((field) => (
              <TableHead className='border text-muted-foreground' key={field.name}>
                {field.name}
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {table.toArray().map((row, idx) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: rows have no stable id
            <TableRow key={idx} className='border'>
              {table.schema.fields.map((field) => {
                const fieldType = getArrowFieldType(field.name, table.schema);
                const value = row[field.name];
                // Explicit per-column format from the app.yml wins; otherwise
                // infer `currency` from column names like `*_sales` /
                // `*_revenue` so existing dashboards get the right formatting
                // without regeneration. The rule is `inferColumnFormat`'s, the
                // one the charts use: a numeric column whose name says money.
                const columnFormat =
                  display.formats?.[field.name] ?? inferColumnFormat(field.name, fieldType);
                // When a format is in play, always route through the
                // currency/percent/number formatter — it handles bigints and
                // stringified numerics uniformly, and decimals given the
                // column type, which is where their scale is. Otherwise fall
                // back to the Arrow-aware value formatter (dates, decimals, …).
                const formattedValue = columnFormat
                  ? formatValue(value, columnFormat, { type: fieldType })
                  : fieldType
                    ? getArrowValueWithType(value, fieldType)
                    : value;
                // `cellText`, as the query-result table prints a cell: a NULL is
                // empty in every column, where `String` wrote "null" in the ones
                // with no format.
                return (
                  <TableCell className='border' key={field.name}>
                    {cellText(formattedValue)}
                  </TableCell>
                );
              })}
            </TableRow>
          ))}
        </TableBody>
      </DataTable>
    );
  }

  return (
    <div className='items-left flex flex-col gap-4' data-testid='app-data-table-block'>
      <h2 className='font-bold text-foreground text-xl'>{display.title}</h2>
      {tableContent}
    </div>
  );
};
