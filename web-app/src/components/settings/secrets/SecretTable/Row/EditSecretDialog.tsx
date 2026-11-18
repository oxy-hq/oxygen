import type React from "react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { useUpdateSecret } from "@/hooks/api/secrets/useSecretMutations";
import { useSharedSecrets } from "@/hooks/api/secrets/useSharedSecrets";
import type { Secret, SecretEditFormData, UpdateSecretRequest } from "@/types/secret";
import { SecretValueField } from "../../CreateApiToken";
import { useMintedApiToken } from "../../CreateApiToken/useMintedApiToken";
import { parseAppSecretName } from "../../UnifiedSecretsTable/appSecretName";

interface EditSecretDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  secret: Secret | null;
  onSecretUpdated: () => void;
}

export const EditSecretDialog: React.FC<EditSecretDialogProps> = ({
  open,
  onOpenChange,
  secret,
  onSecretUpdated
}) => {
  const updateSecretMutation = useUpdateSecret();
  // A token created for this secret: saved as its new value, revoked if the dialog closes unsaved.
  const token = useMintedApiToken();
  const { discard: discardToken } = token;
  // However the dialog closes — its own buttons, Escape, or the parent dropping `open` — a token
  // it did not save goes with it, including one still being minted.
  useEffect(() => {
    if (!open) discardToken();
  }, [open, discardToken]);
  const { data: sharing } = useSharedSecrets(open);
  // An app's own secret is stored as `apps/<app_id>/<KEY>`: the key and the app name the token.
  const scoped = secret ? parseAppSecretName(secret.name) : null;
  const app = scoped ? sharing?.apps.find((a) => a.app_id === scoped.appId) : undefined;
  const [formData, setFormData] = useState<SecretEditFormData>({
    value: "",
    description: ""
  });
  const value = token.minted?.value ?? formData.value ?? "";
  const [errors, setErrors] = useState<{ [key: string]: string }>({});

  // Reset form when secret changes or dialog opens
  useEffect(() => {
    if (open && secret) {
      setFormData({
        value: "",
        description: secret.description || ""
      });
      setErrors({});
    }
  }, [open, secret]);

  const validateForm = (): boolean => {
    const newErrors: { [key: string]: string } = {};

    if (!value.trim()) {
      newErrors.value = "Secret value is required";
    }

    setErrors(newErrors);
    return Object.keys(newErrors).length === 0;
  };

  const handleUpdateSecret = async () => {
    if (!secret || !validateForm()) {
      return;
    }

    try {
      const request: UpdateSecretRequest = {
        value,
        description: formData.description?.trim() || undefined
      };

      await updateSecretMutation.mutateAsync({
        id: secret.id,
        request
      });

      // Saved: the token is the secret's now, not one to take back on close.
      token.settle();
      onSecretUpdated();
    } catch (error) {
      console.error("Failed to update secret:", error);
      // Error toast is handled in the mutation hook
    }
  };

  const handleCancel = () => {
    setFormData({
      value: "",
      description: secret?.description || ""
    });
    setErrors({});
    token.discard();
    onOpenChange(false);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) token.discard();
        onOpenChange(next);
      }}
    >
      <DialogContent className='max-h-[90vh] overflow-y-auto sm:max-w-[425px]'>
        <DialogHeader>
          <DialogTitle>Edit Secret</DialogTitle>
          <DialogDescription>
            {/* An app's own secret is stored as `apps/<app_id>/<KEY>`: name it the way the
                table does, by its key and the app that reads it, never by the storage name. */}
            Update the secret value and description for "{scoped?.key ?? secret?.name}"
            {app ? `, which only ${app.app_name} reads` : ""}.
          </DialogDescription>
        </DialogHeader>

        <div className='grid gap-4 py-4'>
          <div className='grid gap-2'>
            <Label htmlFor='edit-name'>Name</Label>
            <Input
              id='edit-name'
              value={scoped?.key ?? secret?.name ?? ""}
              disabled
              className='bg-muted'
            />
            <p className='text-muted-foreground text-xs'>Secret name cannot be changed</p>
          </div>

          <SecretValueField
            id='edit-value'
            label='New Value *'
            placeholder='Enter new secret value'
            value={formData.value ?? ""}
            onChange={(next) => setFormData((prev) => ({ ...prev, value: next }))}
            error={errors.value}
            token={token}
            secretName={scoped?.key ?? secret?.name ?? ""}
            app={app ? { name: app.app_name, slug: app.app_slug } : undefined}
          />

          <div className='grid gap-2'>
            <Label htmlFor='edit-description'>Description</Label>
            <Textarea
              id='edit-description'
              placeholder='Optional description of this secret'
              value={formData.description}
              onChange={(e) => setFormData({ ...formData, description: e.target.value })}
              rows={3}
            />
          </div>
        </div>

        <DialogFooter>
          <Button variant='outline' onClick={handleCancel}>
            Cancel
          </Button>
          <Button
            onClick={handleUpdateSecret}
            disabled={updateSecretMutation.isPending || token.pending}
          >
            Update Secret
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
