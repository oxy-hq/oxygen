import { useEffect, useId, useState } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { getDuckDB } from "@/libs/duckdb";
import type { ControlConfig, DataContainer } from "@/types/app";
import { getData, registerFromTableData } from "../Displays/utils";

type TableData = { file_path: string; json?: string | null };

type Props = {
  control: ControlConfig;
  value: string;
  data?: DataContainer;
  onChange: (value: string) => void;
};

export function SelectControl({ control, value, data, onChange }: Props) {
  const { project, branchName } = useCurrentProjectBranch();
  const [options, setOptions] = useState<string[]>([]);
  const selectId = useId();

  useEffect(() => {
    // Static options take priority
    if (control.options && control.options.length > 0) {
      setOptions(control.options.map(String));
      return;
    }

    // Dynamic options from a source task result
    if (!control.source || !data) return;

    const tableData = getData(data, control.source) as TableData | null;
    if (!tableData?.file_path) return;

    let cancelled = false;
    void (async () => {
      try {
        const fileName = await registerFromTableData(tableData, project.id, branchName);
        const db = await getDuckDB();
        const connection = await db.connect();

        try {
          const schema = await connection.query(`SELECT * FROM "${fileName}" LIMIT 0`);
          const firstCol = schema.schema.fields[0]?.name;
          if (firstCol) {
            const result = await connection.query(
              `SELECT DISTINCT "${firstCol}" as val FROM "${fileName}" ORDER BY "${firstCol}"`
            );
            const values = result.toArray().map((row) => String(row.val));
            if (!cancelled) setOptions(values);
          }
        } finally {
          await connection.close();
        }
      } catch {
        // silently ignore errors fetching options
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [control.source, control.options, data, project.id, branchName]);

  return (
    <div className='flex flex-col gap-1'>
      {control.label && (
        <label htmlFor={selectId} className='font-medium text-muted-foreground text-xs'>
          {control.label}
        </label>
      )}
      <Select value={value} onValueChange={onChange}>
        <SelectTrigger id={selectId} size='sm' className='h-8 min-w-32'>
          <SelectValue placeholder={control.label ?? control.name} />
        </SelectTrigger>
        <SelectContent>
          {options.map((opt) => (
            <SelectItem key={opt} value={opt}>
              {opt}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
