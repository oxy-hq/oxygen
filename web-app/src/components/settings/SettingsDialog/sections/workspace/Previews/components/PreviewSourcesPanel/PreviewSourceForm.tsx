import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { useUpsertPreviewSource } from "@/hooks/api/workspaces/usePreviews";
import {
  productionRealmMessage,
  productionVarMessage,
  reservedVarMessage,
  rotatingVarTakenMessage
} from "@/libs/utils/preview";
import type { PreviewSourceItem, UpsertPreviewSourceRequest } from "@/types/workspace";

type TokenMode = "refresh" | "access";

interface FormValues {
  pipeline: string;
  realmId: string;
  tokenMode: TokenMode;
  tokenVar: string;
  clientSecretVar: string;
  clientIdVar: string;
}

function defaultsFor(existing: PreviewSourceItem | undefined): FormValues {
  if (!existing) {
    return {
      pipeline: "",
      realmId: "",
      tokenMode: "refresh",
      tokenVar: "",
      clientSecretVar: "",
      clientIdVar: ""
    };
  }
  const { overrides } = existing;
  const tokenMode: TokenMode = overrides.access_token_var ? "access" : "refresh";
  return {
    pipeline: existing.pipeline,
    realmId: overrides.realm_id,
    tokenMode,
    tokenVar:
      (tokenMode === "access" ? overrides.access_token_var : overrides.refresh_token_var) ?? "",
    clientSecretVar: overrides.client_secret_var ?? "",
    clientIdVar: overrides.client_id_var ?? overrides.client_id ?? ""
  };
}

/**
 * Register (or edit) a pipeline's sandbox source: var NAMES only, never a
 * secret's value — each name must already exist as a workspace secret.
 * Shared by "Register a source" (no `existing`) and a row's Edit
 * (pre-filled, pipeline locked since `PUT` upserts by pipeline name).
 */
export default function PreviewSourceForm({
  workspaceId,
  existing,
  onDone
}: {
  workspaceId: string;
  existing?: PreviewSourceItem;
  onDone: () => void;
}) {
  const upsert = useUpsertPreviewSource(workspaceId);
  const {
    register,
    handleSubmit,
    watch,
    setValue,
    setError,
    formState: { errors }
  } = useForm<FormValues>({ defaultValues: defaultsFor(existing) });
  const tokenMode = watch("tokenMode");

  const onSubmit = (values: FormValues) => {
    const pipeline = values.pipeline.trim();
    if (pipeline.startsWith("preview:")) {
      setError("pipeline", {
        type: "value",
        message: 'Pipeline can\'t start with "preview:" — that prefix is reserved.'
      });
      return;
    }
    const overrides: UpsertPreviewSourceRequest["overrides"] = {
      realm_id: values.realmId.trim(),
      ...(values.clientSecretVar.trim()
        ? { client_secret_var: values.clientSecretVar.trim() }
        : {}),
      ...(values.clientIdVar.trim() ? { client_id_var: values.clientIdVar.trim() } : {}),
      ...(values.tokenMode === "refresh"
        ? { refresh_token_var: values.tokenVar.trim() }
        : { access_token_var: values.tokenVar.trim() })
    };
    upsert.mutate(
      { pipeline, environment: "sandbox", overrides },
      {
        onSuccess: onDone,
        onError: (err) => {
          const realmMsg = productionRealmMessage(err);
          if (realmMsg) {
            setError("realmId", { type: "server", message: realmMsg });
            return;
          }
          const varMsg =
            productionVarMessage(err) ?? rotatingVarTakenMessage(err) ?? reservedVarMessage(err);
          if (varMsg) setError("tokenVar", { type: "server", message: varMsg });
        }
      }
    );
  };

  return (
    <form
      onSubmit={handleSubmit(onSubmit)}
      className='flex flex-col gap-2 rounded-md border p-3'
      data-testid='preview-source-form'
      noValidate
    >
      <div className='grid gap-2 sm:grid-cols-2'>
        <div className='flex flex-col gap-1'>
          <Label htmlFor='preview-source-pipeline'>Pipeline</Label>
          <Input
            id='preview-source-pipeline'
            placeholder='quickbooks_financials_eastbay'
            autoComplete='off'
            spellCheck={false}
            className='font-mono'
            disabled={!!existing}
            data-testid='preview-source-pipeline'
            {...register("pipeline", { required: "Enter the pipeline name." })}
          />
          {errors.pipeline && (
            <p
              role='alert'
              className='text-destructive text-xs'
              data-testid='preview-source-pipeline-error'
            >
              {errors.pipeline.message}
            </p>
          )}
        </div>
        <div className='flex flex-col gap-1'>
          <Label htmlFor='preview-source-environment'>Environment</Label>
          <Input id='preview-source-environment' value='sandbox' disabled className='font-mono' />
        </div>
      </div>

      <div className='flex flex-col gap-1'>
        <Label htmlFor='preview-source-realm'>Realm id</Label>
        <Input
          id='preview-source-realm'
          placeholder='4620816365000000'
          className='font-mono'
          data-testid='preview-source-realm'
          {...register("realmId", { required: "Enter the sandbox company's realm id." })}
        />
        {errors.realmId && (
          <p
            role='alert'
            className='text-destructive text-xs'
            data-testid='preview-source-realm-error'
          >
            {errors.realmId.message}
          </p>
        )}
      </div>

      <div className='flex flex-col gap-1'>
        <Label htmlFor='preview-source-token-mode'>Token</Label>
        <Select value={tokenMode} onValueChange={(v) => setValue("tokenMode", v as TokenMode)}>
          <SelectTrigger id='preview-source-token-mode' data-testid='preview-source-token-mode'>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value='refresh'>Refresh token (rotates)</SelectItem>
            <SelectItem value='access'>Access token (static)</SelectItem>
          </SelectContent>
        </Select>
      </div>

      <div className='flex flex-col gap-1'>
        <Label htmlFor='preview-source-token-var'>
          {tokenMode === "refresh" ? "Refresh token var" : "Access token var"}
        </Label>
        <Input
          id='preview-source-token-var'
          placeholder='QB_SANDBOX_REFRESH_TOKEN__EASTBAY'
          autoComplete='off'
          spellCheck={false}
          className='font-mono'
          data-testid='preview-source-token-var'
          {...register("tokenVar", { required: "Enter the secret's name, not its value." })}
        />
        {errors.tokenVar && (
          <p
            role='alert'
            className='text-destructive text-xs'
            data-testid='preview-source-token-var-error'
          >
            {errors.tokenVar.message}
          </p>
        )}
      </div>

      <div className='grid gap-2 sm:grid-cols-2'>
        <div className='flex flex-col gap-1'>
          <Label htmlFor='preview-source-client-secret-var'>Client secret var</Label>
          <Input
            id='preview-source-client-secret-var'
            placeholder='QB_SANDBOX_CLIENT_SECRET'
            autoComplete='off'
            spellCheck={false}
            className='font-mono'
            data-testid='preview-source-client-secret-var'
            {...register("clientSecretVar", {
              validate: (v) =>
                tokenMode === "access" || v.trim() !== "" || "Required with a refresh token."
            })}
          />
          {errors.clientSecretVar && (
            <p
              role='alert'
              className='text-destructive text-xs'
              data-testid='preview-source-client-secret-var-error'
            >
              {errors.clientSecretVar.message}
            </p>
          )}
        </div>
        <div className='flex flex-col gap-1'>
          <Label htmlFor='preview-source-client-id-var'>Client id var (optional)</Label>
          <Input
            id='preview-source-client-id-var'
            placeholder='QB_SANDBOX_CLIENT_ID'
            autoComplete='off'
            spellCheck={false}
            className='font-mono'
            data-testid='preview-source-client-id-var'
            {...register("clientIdVar")}
          />
        </div>
      </div>

      <p className='text-muted-foreground text-xs'>
        Names only — each must already exist as a secret in this workspace's Secrets. The secret's
        value never passes through this form.
      </p>

      <div className='flex justify-end gap-2'>
        <Button
          type='button'
          variant='ghost'
          size='sm'
          onClick={onDone}
          data-testid='preview-source-cancel'
        >
          Cancel
        </Button>
        <Button
          type='submit'
          size='sm'
          disabled={upsert.isPending}
          data-testid='preview-source-submit'
        >
          {upsert.isPending ? "Saving…" : existing ? "Save" : "Register"}
        </Button>
      </div>
    </form>
  );
}
