import { useEffect, useId, useRef, useState } from "react";
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
  // The source file `options` were read from; null when they are not a file's.
  const optionsFile = useRef<string | null>(null);
  // A list that failed to load is not a list with nothing in it: the control says so.
  const [optionsFailed, setOptionsFailed] = useState(false);
  const selectId = useId();

  useEffect(() => {
    setOptionsFailed(false);
    // Static options take priority
    if (control.options && control.options.length > 0) {
      optionsFile.current = null;
      setOptions(control.options.map(String));
      return;
    }

    // Dynamic options from a source task result. What another file listed is not
    // what this one holds, so it is not offered while this one loads, nor at all
    // when there is no file to read. The same file read again (the app re-ran)
    // keeps its options on screen until the new ones arrive.
    const tableData =
      control.source && data ? (getData(data, control.source) as TableData | null) : null;
    const file = tableData?.file_path || null;
    if (!file || optionsFile.current !== file) {
      optionsFile.current = null;
      setOptions([]);
    }
    if (!tableData || !file) return;

    let cancelled = false;
    void (async () => {
      try {
        const fileName = await registerFromTableData(tableData, project.id, branchName);
        const db = await getDuckDB();
        const connection = await db.connect();

        try {
          const schema = await connection.query(`SELECT * FROM "${fileName}" LIMIT 0`);
          const firstCol = schema.schema.fields[0]?.name;
          let values: string[] = [];
          if (firstCol) {
            const result = await connection.query(
              `SELECT DISTINCT "${firstCol}" as val FROM "${fileName}" ORDER BY "${firstCol}"`
            );
            values = result.toArray().map((row) => String(row.val));
          }
          if (!cancelled) {
            optionsFile.current = file;
            setOptions(values);
          }
        } finally {
          await connection.close();
        }
      } catch (error) {
        console.error(`Failed to load the options of control "${control.name}":`, error);
        if (!cancelled) {
          // Whatever an earlier source listed is not what this one holds.
          optionsFile.current = null;
          setOptions([]);
          setOptionsFailed(true);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [control.name, control.source, control.options, data, project.id, branchName]);

  return (
    <div className='flex flex-col gap-1'>
      {control.label && (
        <label htmlFor={selectId} className='font-medium text-muted-foreground text-xs'>
          {control.label}
        </label>
      )}
      {optionsFailed && (
        <p role='alert' className='text-destructive text-xs'>
          Failed to load options
        </p>
      )}
      <Select value={value} onValueChange={onChange}>
        <SelectTrigger
          id={selectId}
          size='sm'
          className='h-8 min-w-32'
          aria-invalid={optionsFailed}
        >
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
