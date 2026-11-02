import { useFormContext } from "react-hook-form";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import type { WarehousesFormData } from "@/types/database";

interface Props {
  index: number;
}

export default function DuckDBForm({ index }: Props) {
  const { register } = useFormContext<WarehousesFormData>();

  return (
    <div className='space-y-4'>
      {/* Offered only in local mode (`libs/utils/cloudConnections`), and on its way out. */}
      <p role='note' className='rounded-md border border-border bg-muted px-3 py-2 text-xs'>
        File DuckDB is deprecated and will be removed. Use Airhouse instead (Settings → Airhouse).
      </p>
      <div className='space-y-2'>
        <Label>File Search Path</Label>
        <Input
          placeholder='/path/to/data'
          {...register(`warehouses.${index}.config.file_search_path` as never)}
        />
      </div>
    </div>
  );
}
