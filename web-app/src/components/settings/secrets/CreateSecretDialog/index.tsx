import type React from "react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { FieldError } from "@/components/ui/shadcn/field";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { useSetWorkspaceAppSecret } from "@/hooks/api/customApps/useAppSecrets";
import { useCreateSecret } from "@/hooks/api/secrets/useSecretMutations";
import { useSharedSecrets, useShareSecret } from "@/hooks/api/secrets/useSharedSecrets";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { validateSecretName } from "@/libs/utils";
import type { SecretFormData } from "@/types/secret";
import { SecretValueField } from "../CreateApiToken";
import { useMintedApiToken } from "../CreateApiToken/useMintedApiToken";
import { type Scope, WhoCanRead } from "./components/WhoCanRead";

interface CreateSecretDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSecretCreated: () => void;
  initialName?: string;
  /** Apps to share the new workspace secret with, ticked on open — from an app
   *  that is missing this key. */
  initialAppIds?: string[];
}

const EMPTY: SecretFormData = { name: "", value: "", description: "" };
const workspaceScope = (appIds: string[] = []): Scope => ({
  kind: "workspace",
  appIds: new Set(appIds)
});

/**
 * Create a secret. By default it is a **workspace** secret, optionally shared
 * with apps in the same step — one value for every app that needs it. "One app
 * only" stores the app's own `apps/<app_id>/<KEY>` instead, through the app
 * route (the project endpoint rejects the `/` such a name needs).
 */
export const CreateSecretDialog: React.FC<CreateSecretDialogProps> = ({
  open,
  onOpenChange,
  onSecretCreated,
  initialName,
  initialAppIds
}) => {
  const { project } = useCurrentProjectBranch();
  // This dialog says what happened once, for create and share together.
  const createSecret = useCreateSecret({ toastOnSuccess: false });
  const share = useShareSecret({ toastOnSuccess: false });
  const setAppSecret = useSetWorkspaceAppSecret(project.id);
  const { data: sharing } = useSharedSecrets(open);
  const apps = sharing?.apps ?? [];
  // A token created for this secret: saved as its value, revoked if the dialog closes unsaved.
  const token = useMintedApiToken();
  const { discard: discardToken } = token;
  // However the dialog closes — its own buttons, Escape, or the parent dropping `open` — a token
  // it did not save goes with it, including one still being minted.
  useEffect(() => {
    if (!open) discardToken();
  }, [open, discardToken]);

  const [formData, setFormData] = useState<SecretFormData>({ ...EMPTY, name: initialName ?? "" });
  const [scope, setScope] = useState<Scope>(workspaceScope(initialAppIds));
  const [errors, setErrors] = useState<{ [key: string]: string }>({});
  const pending =
    createSecret.isPending || share.isPending || setAppSecret.isPending || token.pending;
  const value = token.minted?.value ?? formData.value;
  const forApp = scope.kind === "app" ? apps.find((a) => a.app_id === scope.appId) : undefined;

  const close = (next: boolean) => {
    if (!next) token.discard();
    onOpenChange(next);
  };

  // Each open starts from what the caller asked for.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset on open, not on a new array identity
  useEffect(() => {
    if (open) {
      setFormData({ ...EMPTY, name: initialName ?? "" });
      setScope(workspaceScope(initialAppIds));
      setErrors({});
    }
  }, [open, initialName]);

  const validateForm = (): boolean => {
    const next: { [key: string]: string } = {};
    const nameValidation = validateSecretName(formData.name);
    if (!nameValidation.isValid) next.name = nameValidation.error ?? "Invalid name";
    if (!value.trim()) next.value = "Secret value is required";
    setErrors(next);
    return Object.keys(next).length === 0;
  };

  const handleCreate = async () => {
    if (!validateForm()) return;
    const name = formData.name.trim();
    try {
      if (scope.kind === "app") {
        // The hook says whether it created or rotated the key.
        await setAppSecret.mutateAsync({ appId: scope.appId, key: name, value });
      } else {
        await createWorkspaceSecret(name, [...scope.appIds]);
      }
      // Saved: the token is the secret's now, not one to take back on close.
      token.settle();
      onSecretCreated();
    } catch (error) {
      // The mutation hooks toast their own failures; the dialog stays open.
      console.error("Failed to create secret:", error);
    }
  };

  const createWorkspaceSecret = async (name: string, appIds: string[]) => {
    const created = await createSecret.mutateAsync({
      name,
      value,
      description: formData.description?.trim() || undefined
    });
    if (appIds.length === 0) {
      toast.success("Secret created successfully");
      return;
    }
    try {
      await share.mutateAsync({ secretId: created.id, name, appIds });
      toast.success(
        `Secret created and shared with ${appIds.length} app${appIds.length === 1 ? "" : "s"}`
      );
    } catch {
      // Created, not shared: the share hook said why. The row exists, so it can
      // be shared from the table — close rather than offer a duplicate create.
      toast.success("Secret created successfully");
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className='max-h-[90vh] overflow-y-auto sm:max-w-md'>
        <DialogHeader>
          <DialogTitle>New secret</DialogTitle>
          <DialogDescription>
            Stored encrypted. Its value is never shown again unless revealed.
          </DialogDescription>
        </DialogHeader>

        <div className='grid gap-4 py-2'>
          <div className='grid gap-2'>
            <Label htmlFor='name'>Name *</Label>
            <Input
              id='name'
              placeholder='e.g. SLACK_BOT_TOKEN'
              value={formData.name}
              onChange={(e) => setFormData({ ...formData, name: e.target.value })}
              className={errors.name ? "border-destructive" : ""}
            />
            {errors.name && <FieldError>{errors.name}</FieldError>}
          </div>

          <SecretValueField
            id='value'
            label='Value *'
            placeholder='Enter secret value'
            value={formData.value}
            onChange={(next) => setFormData((prev) => ({ ...prev, value: next }))}
            error={errors.value}
            token={token}
            secretName={formData.name}
            app={forApp ? { name: forApp.app_name, slug: forApp.app_slug } : undefined}
          />

          <WhoCanRead
            apps={apps}
            name={formData.name}
            scope={scope}
            onScope={setScope}
            onCopy={(value) => setFormData((prev) => ({ ...prev, value }))}
          />

          {scope.kind === "workspace" && (
            <div className='grid gap-2'>
              <Label htmlFor='description'>Description</Label>
              <Textarea
                id='description'
                placeholder='What it is and where it came from'
                value={formData.description}
                onChange={(e) => setFormData({ ...formData, description: e.target.value })}
                rows={2}
              />
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant='outline' onClick={() => close(false)}>
            Cancel
          </Button>
          <Button onClick={handleCreate} disabled={pending} data-testid='secret-create-submit'>
            {pending ? <Spinner /> : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
