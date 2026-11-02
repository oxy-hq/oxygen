import { useFormContext } from "react-hook-form";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Textarea } from "@/components/ui/shadcn/textarea";
import type { WarehousesFormData } from "@/types/database";

interface Props {
  index: number;
  /**
   * A cloud deployment: the key is named by a secret (`key_json_var`), never
   * pasted into a key file on the server's disk. See `libs/utils/cloudConnections`.
   */
  cloud?: boolean;
}

export default function BigQueryForm({ index, cloud = false }: Props) {
  const { register } = useFormContext<WarehousesFormData>();

  return (
    <div className='space-y-4'>
      {cloud ? (
        <div className='space-y-2'>
          <Label htmlFor={`warehouses-${index}-key-json-var`}>Service Account Key Secret</Label>
          <Input
            id={`warehouses-${index}-key-json-var`}
            placeholder='BIGQUERY_KEY_JSON'
            {...register(`warehouses.${index}.config.key_json_var` as never)}
          />
          <p className='text-muted-foreground text-xs'>
            The name of a secret holding the service account key JSON. Add the key under Settings →
            Secrets first.
          </p>
        </div>
      ) : (
        <div className='space-y-2'>
          <Label>Service Account Key (JSON)</Label>
          <Textarea
            placeholder='{"type": "service_account", ...}'
            rows={6}
            {...register(`warehouses.${index}.config.key` as never)}
          />
        </div>
      )}
      <div className='space-y-2'>
        <Label>Dataset</Label>
        <Input
          placeholder='my_dataset'
          {...register(`warehouses.${index}.config.dataset` as never)}
        />
      </div>
      <div className='space-y-2'>
        <Label>Dry Run Limit</Label>
        <Input
          type='number'
          placeholder='1000'
          {...register(`warehouses.${index}.config.dry_run_limit` as never, {
            valueAsNumber: true
          })}
        />
      </div>
    </div>
  );
}
