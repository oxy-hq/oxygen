import { type FormEvent, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { useCreateOidcIssuer } from "@/hooks/api/orgApiAccess";
import type { IssuerKeySource } from "@/types/orgApiAccess";
import { FormField } from "../shared/FormField";
import { describeApiError } from "../utils/errors";
import {
  buildCreateIssuerRequest,
  emptyIssuerForm,
  type IssuerFormState,
  type IssuerPreset,
  PRESETS,
  validateIssuerForm
} from "../utils/issuerTrustForm";

const KEY_SOURCES: { value: IssuerKeySource; label: string }[] = [
  { value: "discovery", label: "Read through discovery" },
  { value: "jwks_uri", label: "From a key set URL" },
  { value: "inline", label: "Paste the keys" }
];

/** What each preset asks for the issuer URL, and where to find it. */
const ISSUER_HINT: Record<IssuerPreset, string> = {
  aws: "The account's issuer URL. `aws iam enable-outbound-web-identity-federation` turns it on, and `aws iam get-outbound-web-identity-federation-info` shows it.",
  custom: "Exactly as its tokens carry it in 'iss'. A trailing slash is ignored."
};

const ISSUER_PLACEHOLDER: Record<IssuerPreset, string> = {
  aws: "https://abc123-def456.tokens.sts.global.api.aws",
  custom: "https://gitlab.example.com"
};

export function IssuerDialog({
  open,
  onOpenChange,
  orgId
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className='max-h-[90vh] overflow-y-auto sm:max-w-lg'
        data-testid='api-access-issuer-dialog'
      >
        <DialogHeader>
          <DialogTitle className='text-base'>Register an OIDC issuer</DialogTitle>
          <DialogDescription className='text-xs'>
            Registering an issuer grants nothing by itself: a trust policy says which of its tokens,
            by their claims, may act as a service account.
          </DialogDescription>
        </DialogHeader>
        {open && <IssuerForm orgId={orgId} onDone={() => onOpenChange(false)} />}
      </DialogContent>
    </Dialog>
  );
}

function IssuerForm({ orgId, onDone }: { orgId: string; onDone: () => void }) {
  const create = useCreateOidcIssuer();
  const [form, setForm] = useState<IssuerFormState>(() => emptyIssuerForm("aws"));
  const [touched, setTouched] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);
  const errors = validateIssuerForm(form);
  const shown = touched ? errors : {};
  const set = (patch: Partial<IssuerFormState>) => setForm((f) => ({ ...f, ...patch }));

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (Object.keys(errors).length > 0 || create.isPending) return;
    setServerError(null);
    try {
      await create.mutateAsync({ orgId, request: buildCreateIssuerRequest(form) });
      toast.success("Registered the issuer");
      onDone();
    } catch (err) {
      setServerError(describeApiError(err, "Couldn't register the issuer."));
    }
  };

  return (
    <form onSubmit={handleSubmit} className='flex min-w-0 flex-col gap-4 pt-1' noValidate>
      <FormField
        htmlFor='api-access-issuer-preset'
        label='Kind'
        hint={PRESETS.find((p) => p.value === form.preset)?.description}
      >
        <Select
          value={form.preset}
          onValueChange={(value) => {
            const preset = value as IssuerPreset;
            setForm((f) => ({ ...emptyIssuerForm(preset), name: f.name, issuer: f.issuer }));
          }}
        >
          <SelectTrigger
            id='api-access-issuer-preset'
            className='text-xs'
            data-testid='api-access-issuer-preset'
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {PRESETS.map((preset) => (
              <SelectItem key={preset.value} value={preset.value} className='text-xs'>
                {preset.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </FormField>

      <FormField htmlFor='api-access-issuer-name' label='Name' error={shown.name}>
        <Input
          id='api-access-issuer-name'
          value={form.name}
          onChange={(e) => set({ name: e.target.value })}
          placeholder={form.preset === "aws" ? "oxy-management" : "gitlab"}
          className='text-xs'
          autoComplete='off'
          autoFocus
          aria-invalid={!!shown.name}
          data-testid='api-access-issuer-name'
        />
      </FormField>

      <FormField
        htmlFor='api-access-issuer-url'
        label='Issuer URL'
        error={shown.issuer}
        hint={ISSUER_HINT[form.preset]}
      >
        <Input
          id='api-access-issuer-url'
          value={form.issuer}
          onChange={(e) => set({ issuer: e.target.value })}
          placeholder={ISSUER_PLACEHOLDER[form.preset]}
          className='font-mono text-xs'
          autoComplete='off'
          spellCheck={false}
          aria-invalid={!!shown.issuer}
          data-testid='api-access-issuer-url'
        />
      </FormField>

      {form.preset === "aws" ? (
        <p
          className='text-muted-foreground text-xs leading-relaxed'
          data-testid='api-access-issuer-aws-note'
        >
          Oxygen reads the issuer's keys through its discovery document, which STS publishes. Each
          token may be exchanged once: the workload asks STS for a new one each time.
        </p>
      ) : (
        <>
          <FormField
            htmlFor='api-access-issuer-key-source'
            label='Signing keys'
            hint={
              form.keySource === "inline"
                ? "Oxygen never fetches anything for this issuer, so its URL may be private. New keys mean registering it again."
                : "Read over public HTTPS when you save, and again when a token names a key Oxygen doesn't hold."
            }
          >
            <Select
              value={form.keySource}
              onValueChange={(value) => set({ keySource: value as IssuerKeySource })}
            >
              <SelectTrigger
                id='api-access-issuer-key-source'
                className='text-xs'
                data-testid='api-access-issuer-key-source'
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {KEY_SOURCES.map((source) => (
                  <SelectItem key={source.value} value={source.value} className='text-xs'>
                    {source.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </FormField>

          {form.keySource === "discovery" && (
            <FormField
              htmlFor='api-access-issuer-discovery'
              label='Discovery URL'
              aside='optional'
              error={shown.discoveryUrl}
              hint='Where to read its discovery document, when not from the issuer URL itself.'
            >
              <Input
                id='api-access-issuer-discovery'
                value={form.discoveryUrl}
                onChange={(e) => set({ discoveryUrl: e.target.value })}
                className='font-mono text-xs'
                autoComplete='off'
                spellCheck={false}
                aria-invalid={!!shown.discoveryUrl}
                data-testid='api-access-issuer-discovery'
              />
            </FormField>
          )}

          {form.keySource === "jwks_uri" && (
            <FormField
              htmlFor='api-access-issuer-jwks-uri'
              label='Key set URL'
              error={shown.jwksUri}
            >
              <Input
                id='api-access-issuer-jwks-uri'
                value={form.jwksUri}
                onChange={(e) => set({ jwksUri: e.target.value })}
                placeholder='https://issuer.example.com/keys'
                className='font-mono text-xs'
                autoComplete='off'
                spellCheck={false}
                aria-invalid={!!shown.jwksUri}
                data-testid='api-access-issuer-jwks-uri'
              />
            </FormField>
          )}

          {form.keySource === "inline" && (
            <FormField
              htmlFor='api-access-issuer-jwks'
              label='Public keys (JSON Web Key Set)'
              error={shown.jwks}
              hint={"The issuer's key set: an object with a 'keys' list."}
            >
              <Textarea
                id='api-access-issuer-jwks'
                value={form.jwksText}
                onChange={(e) => set({ jwksText: e.target.value })}
                rows={5}
                className='font-mono text-xs'
                spellCheck={false}
                aria-invalid={!!shown.jwks}
                data-testid='api-access-issuer-jwks'
              />
            </FormField>
          )}

          <label className='flex items-start gap-2 text-xs' htmlFor='api-access-issuer-jti'>
            <Checkbox
              id='api-access-issuer-jti'
              checked={form.jtiSingleUse}
              onCheckedChange={(checked) => set({ jtiSingleUse: checked === true })}
              data-testid='api-access-issuer-jti'
            />
            <span>
              Each token may be exchanged once{" "}
              <span className='text-muted-foreground'>
                (its <code className='font-mono'>jti</code> is spent). Turn off only for an issuer
                that hands one token out more than once.
              </span>
            </span>
          </label>
        </>
      )}

      {serverError && (
        <p className='text-destructive text-xs' role='alert' data-testid='api-access-issuer-error'>
          {serverError}
        </p>
      )}
      <div className='flex justify-end gap-2'>
        <Button type='button' variant='outline' size='sm' onClick={onDone}>
          Cancel
        </Button>
        <Button
          type='submit'
          size='sm'
          disabled={create.isPending}
          data-testid='api-access-issuer-submit'
        >
          {create.isPending ? "Checking..." : "Register issuer"}
        </Button>
      </div>
    </form>
  );
}
