import type React from "react";
import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
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
import useUpdatePackBody from "@/hooks/api/cameras/useUpdatePackBody";
import type { DomainPack, DomainPackBody } from "@/services/api/cameras";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

type Props = {
  pack: DomainPack | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

/**
 * Operator-facing pack body editor. Renders the active pack's roles
 * as per-role textarea cards and the pack-wide variables as a JSON
 * blob. Save round-trips the full body to `PUT /camera-packs/{key}/body`;
 * the backend renders every template against a dummy context first,
 * so a typo returns 400 and we surface the message via toast without
 * closing the dialog.
 *
 * **Why a single Save button** (vs per-row save): pack edits often
 * span multiple roles (variable change → tweak each prompt that
 * references it). Atomic save matches that automation and avoids
 * partial-state weirdness where roles A+B reference a variable C
 * but C hasn't been saved yet.
 *
 * **Why variables-as-JSON**: v1 — keeps the surface area small.
 * Most real edits are prompt-side; variables change once per pack
 * lifetime. A future iteration can swap this for a key/value
 * form if the JSON blob proves too sharp an edge.
 */
const PackEditorDialog: React.FC<Props> = ({ pack, open, onOpenChange }) => {
  const { workspace } = useCurrentWorkspace();
  const mutation = useUpdatePackBody(workspace?.id);

  // Local draft state that diverges from the server during editing.
  // Reset on every open so a previous-pack draft doesn't leak into
  // a freshly-opened pack.
  const [draft, setDraft] = useState<DomainPackBody | null>(null);
  const [varsText, setVarsText] = useState<string>("{}");
  const [varsError, setVarsError] = useState<string | null>(null);

  useEffect(() => {
    if (!open || !pack) return;
    setDraft(structuredClone(pack.body));
    setVarsText(JSON.stringify(pack.body.variables ?? {}, null, 2));
    setVarsError(null);
  }, [open, pack]);

  const setRoleField = (
    idx: number,
    field: "role_key" | "label" | "hint" | "prompt",
    value: string
  ) => {
    setDraft((prev) =>
      prev
        ? {
            ...prev,
            roles: prev.roles.map((r, i) => (i === idx ? { ...r, [field]: value } : r))
          }
        : prev
    );
  };

  const tryParseVars = (): Record<string, unknown> | null => {
    try {
      const parsed = JSON.parse(varsText);
      if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
        setVarsError("Variables must be a JSON object.");
        return null;
      }
      setVarsError(null);
      return parsed as Record<string, unknown>;
    } catch (err) {
      setVarsError(err instanceof Error ? err.message : "Invalid JSON");
      return null;
    }
  };

  const onSave = async () => {
    if (!pack || !draft) return;
    const vars = tryParseVars();
    if (vars === null) return;
    try {
      await mutation.mutateAsync({
        packKey: pack.pack_key,
        body: { ...draft, variables: vars }
      });
      toast.success("Domain pack updated");
      onOpenChange(false);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "save failed";
      toast.error(`Could not save pack: ${msg}`);
    }
  };

  // Available template variables — copy-into-prompt hint shown
  // under each role's textarea. Computed once per dialog mount,
  // cheap enough not to memo aggressively.
  const variableHints = useMemo(() => {
    const base = ["track_id", "dwell_seconds"];
    if (!draft) return base;
    const varKeys = Object.keys(draft.variables ?? {});
    return [...base, ...varKeys.map((k) => `variables.${k}`)];
  }, [draft]);

  if (!draft) {
    return (
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className={`${DESIGN_REFERENCE} max-w-4xl`} />
      </Dialog>
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={`${DESIGN_REFERENCE} max-h-[90vh] max-w-4xl overflow-y-auto`}>
        <DialogHeader>
          <DialogTitle>Edit pack · {pack?.name}</DialogTitle>
          <DialogDescription>
            Edit role prompts and pack variables. Templates are Jinja2 — typos are caught
            server-side and rejected before workers see them.
          </DialogDescription>
        </DialogHeader>

        <div className='flex flex-col gap-5'>
          {/* Variables card */}
          <div className='flex flex-col gap-2 border border-border p-3'>
            <div className='flex items-center justify-between'>
              <Label htmlFor='pack-variables' className='font-medium'>
                Variables
              </Label>
              <Badge variant='secondary'>JSON</Badge>
            </div>
            <Textarea
              id='pack-variables'
              value={varsText}
              onChange={(e) => setVarsText(e.target.value)}
              className='font-mono text-xs'
              rows={6}
              spellCheck={false}
            />
            {varsError ? (
              <span className='text-destructive text-xs'>{varsError}</span>
            ) : (
              <span className='text-muted-foreground text-xs'>
                Available in every role's prompt as <code>{`{{ variables.<key> }}`}</code>.
              </span>
            )}
          </div>

          {/* Roles */}
          <div className='flex flex-col gap-3'>
            <Label className='font-medium'>Roles</Label>
            {draft.roles.map((role, idx) => (
              <div
                key={role.role_key || `role-${idx}`}
                className='flex flex-col gap-2 border border-border p-3'
              >
                <div className='grid gap-2 sm:grid-cols-2'>
                  <div className='flex flex-col gap-1'>
                    <Label htmlFor={`role-key-${idx}`} className='text-xs'>
                      Key
                    </Label>
                    <Input
                      id={`role-key-${idx}`}
                      value={role.role_key}
                      onChange={(e) => setRoleField(idx, "role_key", e.target.value)}
                    />
                  </div>
                  <div className='flex flex-col gap-1'>
                    <Label htmlFor={`role-label-${idx}`} className='text-xs'>
                      Label
                    </Label>
                    <Input
                      id={`role-label-${idx}`}
                      value={role.label}
                      onChange={(e) => setRoleField(idx, "label", e.target.value)}
                    />
                  </div>
                </div>
                <div className='flex flex-col gap-1'>
                  <Label htmlFor={`role-hint-${idx}`} className='text-xs'>
                    Hint
                  </Label>
                  <Input
                    id={`role-hint-${idx}`}
                    value={role.hint}
                    onChange={(e) => setRoleField(idx, "hint", e.target.value)}
                  />
                </div>
                <div className='flex flex-col gap-1'>
                  <Label htmlFor={`role-prompt-${idx}`} className='text-xs'>
                    Prompt template
                  </Label>
                  <Textarea
                    id={`role-prompt-${idx}`}
                    value={role.prompt}
                    onChange={(e) => setRoleField(idx, "prompt", e.target.value)}
                    rows={12}
                    className='font-mono text-xs'
                    spellCheck={false}
                  />
                  <div className='flex flex-wrap gap-1 text-xs'>
                    <span className='text-muted-foreground'>Available:</span>
                    {variableHints.map((v) => (
                      <code key={v} className='bg-muted px-1.5 py-0.5 text-muted-foreground'>
                        {`{{ ${v} }}`}
                      </code>
                    ))}
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>

        <DialogFooter>
          <Button
            variant='outline'
            onClick={() => onOpenChange(false)}
            disabled={mutation.isPending}
          >
            Cancel
          </Button>
          <Button onClick={onSave} disabled={mutation.isPending}>
            {mutation.isPending ? "Saving…" : "Save pack"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default PackEditorDialog;
