import { useFormContext } from "react-hook-form";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import type { WarehousesFormData } from "@/types/database";

interface Props {
  index: number;
  /**
   * A cloud deployment: a private key is named by a secret holding the PEM
   * (`private_key_var`), never a key file on the server's disk. See
   * `libs/utils/cloudConnections`.
   */
  cloud?: boolean;
}

export default function SnowflakeForm({ index, cloud = false }: Props) {
  const { register, watch, setValue } = useFormContext<WarehousesFormData>();

  const authMode =
    (watch(`warehouses.${index}.config.auth_mode` as never) as unknown as string) || "password";

  return (
    <div className='space-y-4'>
      <div className='space-y-2'>
        <Label>Account</Label>
        <Input
          placeholder='myorg-myaccount'
          {...register(`warehouses.${index}.config.account` as never)}
        />
      </div>
      <div className='grid grid-cols-2 gap-4'>
        <div className='space-y-2'>
          <Label>Database</Label>
          <Input
            placeholder='MY_DATABASE'
            {...register(`warehouses.${index}.config.database` as never)}
          />
        </div>
        <div className='space-y-2'>
          <Label>Schema</Label>
          <Input placeholder='PUBLIC' {...register(`warehouses.${index}.config.schema` as never)} />
        </div>
      </div>
      <div className='grid grid-cols-2 gap-4'>
        <div className='space-y-2'>
          <Label>Warehouse</Label>
          <Input
            placeholder='COMPUTE_WH'
            {...register(`warehouses.${index}.config.warehouse` as never)}
          />
        </div>
        <div className='space-y-2'>
          <Label>Role</Label>
          <Input
            placeholder='ACCOUNTADMIN'
            {...register(`warehouses.${index}.config.role` as never)}
          />
        </div>
      </div>
      <div className='space-y-2'>
        <Label>Auth Mode</Label>
        <Select
          value={authMode}
          onValueChange={(value) =>
            setValue(`warehouses.${index}.config.auth_mode` as never, value as never)
          }
        >
          <SelectTrigger>
            <SelectValue placeholder='Select auth mode' />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value='password'>Password</SelectItem>
            <SelectItem value='browser'>Browser SSO</SelectItem>
            <SelectItem value='private_key'>Private Key</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <div className='space-y-2'>
        <Label>Username</Label>
        <Input placeholder='myuser' {...register(`warehouses.${index}.config.username` as never)} />
      </div>
      {authMode === "password" && (
        <>
          <div className='space-y-2'>
            <Label>Password</Label>
            <Input
              type='password'
              placeholder='password'
              {...register(`warehouses.${index}.config.password` as never)}
            />
          </div>
          <div className='space-y-2'>
            <Label>Password Secret Variable</Label>
            <Input
              placeholder='MY_SNOWFLAKE_PASSWORD'
              {...register(`warehouses.${index}.config.password_var` as never)}
            />
          </div>
        </>
      )}
      {authMode === "private_key" &&
        (cloud ? (
          <div className='space-y-2'>
            <Label htmlFor={`warehouses-${index}-private-key-var`}>Private Key Secret</Label>
            <Input
              id={`warehouses-${index}-private-key-var`}
              placeholder='SNOWFLAKE_PRIVATE_KEY'
              {...register(`warehouses.${index}.config.private_key_var` as never)}
            />
            <p className='text-muted-foreground text-xs'>
              The name of a secret holding the PEM private key. Add the key under Settings → Secrets
              first.
            </p>
          </div>
        ) : (
          <div className='space-y-2'>
            <Label>Private Key Path</Label>
            <Input
              placeholder='/path/to/rsa_key.p8'
              {...register(`warehouses.${index}.config.private_key_path` as never)}
            />
          </div>
        ))}
    </div>
  );
}
