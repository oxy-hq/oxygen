import { formatDistanceToNow } from "date-fns";
import { Check, Code2, Copy, Edit, Eye, EyeOff, Trash2, UserRound } from "lucide-react";
import type React from "react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { cn } from "@/libs/shadcn/utils";
import { SecretService } from "@/services/secretService";
import type { Secret } from "@/types/secret";
import { DOTS, SOURCE_CONFIG, type UnifiedRow } from "../types";

interface SecretDetailDialogProps {
  row: UnifiedRow | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onEdit: (secret: Secret) => void;
  onDelete: (secret: Secret) => void;
}

const DetailField: React.FC<React.PropsWithChildren<{ label: string }>> = ({ label, children }) => (
  <div className='flex items-start gap-3 border-border/60 border-b py-3 last:border-0'>
    <span className='w-20 shrink-0 pt-0.5 text-muted-foreground text-xs uppercase tracking-wide'>
      {label}
    </span>
    <div className='min-w-0 flex-1 text-sm'>{children}</div>
  </div>
);

/**
 * Detail view for a single secret. Surfaces everything that used to
 * live in the (now removed) Value / Updated / Actions table columns: the
 * masked value with reveal + copy, source, last-updated metadata, and the
 * edit / delete actions.
 */
export const SecretDetailDialog: React.FC<SecretDetailDialogProps> = ({
  row,
  open,
  onOpenChange,
  onEdit,
  onDelete
}) => {
  const { project } = useCurrentProjectBranch();
  const [revealed, setRevealed] = useState<string | undefined>(undefined);
  const [revealLoading, setRevealLoading] = useState(false);
  const [copied, setCopied] = useState(false);

  // Reset transient state whenever the target row changes or the dialog closes.
  // row.key/open are the intended reset triggers even though the body only calls
  // (stable) setters.
  // biome-ignore lint/correctness/useExhaustiveDependencies: intentional reset triggers
  useEffect(() => {
    setRevealed(undefined);
    setRevealLoading(false);
    setCopied(false);
  }, [row?.key, open]);

  if (!row) {
    return null;
  }

  const sourceConfig = SOURCE_CONFIG[row.source];
  const isRevealed = revealed !== undefined;
  const date = row.secretInfo?.updated_at;
  const updatedBy = row.secretInfo?.updated_by_email ?? row.secretInfo?.created_by_email;
  const displayValue = isRevealed ? revealed : DOTS;

  const handleReveal = async () => {
    if (isRevealed) {
      setRevealed(undefined);
      return;
    }
    setRevealLoading(true);
    try {
      const value = await SecretService.revealSecret(project.id, row.secretInfo.id);
      setRevealed(value);
    } catch {
      toast.error("Failed to reveal secret value");
    } finally {
      setRevealLoading(false);
    }
  };

  const handleCopy = async () => {
    let value = revealed;
    if (value === undefined) {
      try {
        value = await SecretService.revealSecret(project.id, row.secretInfo.id);
      } catch {
        toast.error("Failed to copy secret value");
        return;
      }
    }
    try {
      await navigator.clipboard.writeText(value);
    } catch {
      toast.error("Failed to copy to clipboard");
      return;
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-md'>
        <DialogHeader>
          <DialogTitle className='flex items-center gap-2 pr-6'>
            <Code2 className='size-4 shrink-0 text-muted-foreground/60' />
            <span className='min-w-0 break-all font-mono text-base'>{row.name}</span>
          </DialogTitle>
        </DialogHeader>

        <div className='flex flex-col'>
          <DetailField label='Value'>
            <div className='flex items-center gap-2'>
              <span
                className={cn(
                  "min-w-0 flex-1 break-all font-mono text-sm",
                  isRevealed ? "text-foreground" : "text-muted-foreground/70 tracking-widest"
                )}
              >
                {displayValue}
              </span>
              <div className='-mr-1 flex shrink-0 items-center gap-0.5'>
                <Button
                  variant='ghost'
                  size='sm'
                  className='size-7 p-0'
                  onClick={handleReveal}
                  disabled={revealLoading}
                  title={isRevealed ? "Hide value" : "Reveal value"}
                >
                  {isRevealed ? <EyeOff className='size-3.5' /> : <Eye className='size-3.5' />}
                </Button>
                <Button
                  variant='ghost'
                  size='sm'
                  className='size-7 p-0'
                  onClick={handleCopy}
                  title='Copy value'
                >
                  {copied ? (
                    <Check className='size-3.5 text-success' />
                  ) : (
                    <Copy className='size-3.5' />
                  )}
                </Button>
              </div>
            </div>
          </DetailField>

          <DetailField label='Source'>
            <Badge
              variant='outline'
              className={cn("font-medium text-[10px]", sourceConfig.className)}
            >
              {sourceConfig.label}
            </Badge>
          </DetailField>

          {row.secretInfo?.description && (
            <DetailField label='Description'>
              <span className='text-muted-foreground'>{row.secretInfo.description}</span>
            </DetailField>
          )}

          {date && (
            <DetailField label='Updated'>
              <div className='flex flex-col gap-0.5 text-muted-foreground'>
                <span>{formatDistanceToNow(new Date(date), { addSuffix: true })}</span>
                {updatedBy && (
                  <span className='flex items-center gap-1 text-muted-foreground/70 text-xs'>
                    <UserRound className='size-3 shrink-0' />
                    <span className='break-all'>{updatedBy}</span>
                  </span>
                )}
              </div>
            </DetailField>
          )}
        </div>

        <DialogFooter className='sm:justify-between'>
          <Button
            variant='ghost'
            size='sm'
            className='text-destructive hover:bg-destructive/10 hover:text-destructive'
            onClick={() => onDelete(row.secretInfo)}
          >
            <Trash2 className='size-4' />
            Delete
          </Button>
          <Button size='sm' onClick={() => onEdit(row.secretInfo)}>
            <Edit className='size-4' />
            Edit
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
