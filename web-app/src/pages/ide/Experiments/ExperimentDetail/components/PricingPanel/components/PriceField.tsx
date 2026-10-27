import type { ReactNode } from "react";
import { FieldError } from "@/components/ui/shadcn/field";
import { Label } from "@/components/ui/shadcn/label";

/** A label, an input slot, and either the inline error or a hint under it. */
export default function PriceField({
  id,
  label,
  hint,
  error,
  children
}: {
  id: string;
  label: string;
  hint?: string;
  error?: string;
  children: ReactNode;
}) {
  return (
    <div className='flex flex-col gap-1.5'>
      <Label htmlFor={id}>{label}</Label>
      {children}
      {error ? (
        <FieldError data-testid={`${id}-error`}>{error}</FieldError>
      ) : (
        hint && <p className='text-muted-foreground text-xs'>{hint}</p>
      )}
    </div>
  );
}
