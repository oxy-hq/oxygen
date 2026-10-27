import type { ReactNode } from "react";
import type { Path, UseFormReturn } from "react-hook-form";
import { FieldError } from "@/components/ui/shadcn/field";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { cn } from "@/libs/shadcn/utils";
import type { DesignFormValues } from "../designForm";

/** What every section of the form is handed. */
export interface SectionProps {
  form: UseFormReturn<DesignFormValues>;
  /** The message to show beside a field (form path such as `waves.0.units`), if any. */
  messageFor: (name: string) => string | undefined;
}

/** `waves.0.switchOn` -> `experiment-design-waves-0-switch-on`: the field's id and testid. */
export const fieldId = (name: string) =>
  `experiment-design-${name.replace(/\./g, "-").replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`;

/** A label, an input slot, and either the inline error or a hint under it. */
export function DesignField({
  id,
  labelFor,
  label,
  hint,
  error,
  className,
  children
}: {
  id: string;
  /** The control the label names, when that is not the field's own id. */
  labelFor?: string;
  label: string;
  hint?: ReactNode;
  error?: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={cn("flex flex-col gap-1.5", className)}>
      <Label htmlFor={labelFor ?? id}>{label}</Label>
      {children}
      {error ? (
        <FieldError data-testid={`${id}-error`}>{error}</FieldError>
      ) : (
        hint && <p className='text-muted-foreground text-xs'>{hint}</p>
      )}
    </div>
  );
}

type TextProps = SectionProps & {
  name: Path<DesignFormValues>;
  label: string;
  hint?: ReactNode;
  placeholder?: string;
  multiline?: boolean;
  type?: "text" | "date" | "datetime-local" | "number";
  /** For a number: the bounds and the step (`"any"` for a decimal). */
  min?: number;
  max?: number;
  step?: number | "any";
  readOnly?: boolean;
  /** The id of a `<datalist>` of suggestions. */
  list?: string;
  className?: string;
};

/** A text, number or date input bound to one form field. */
export function TextField(props: TextProps) {
  const { form, messageFor, name, label, hint, multiline, className, ...input } = props;
  const id = fieldId(name);
  const error = messageFor(name);
  const shared = {
    id,
    "data-testid": id,
    "aria-invalid": Boolean(error),
    placeholder: input.placeholder,
    readOnly: input.readOnly,
    ...form.register(name)
  };
  return (
    <DesignField id={id} label={label} hint={hint} error={error} className={className}>
      {multiline ? (
        <Textarea rows={2} {...shared} />
      ) : (
        <Input
          type={input.type ?? "text"}
          min={input.min}
          max={input.max}
          step={input.step}
          list={input.list}
          {...shared}
        />
      )}
    </DesignField>
  );
}

export interface Choice<T extends string> {
  value: T;
  label: string;
}

/** A small fixed choice as radios: visible, keyboard-reachable, one click. */
export function ChoiceField<T extends string>({
  form,
  messageFor,
  name,
  label,
  choices
}: SectionProps & { name: Path<DesignFormValues>; label: string; choices: Choice<T>[] }) {
  const id = fieldId(name);
  const error = messageFor(name);
  return (
    <div className='flex flex-col gap-1.5'>
      <span className='font-medium text-sm'>{label}</span>
      <div role='radiogroup' aria-label={label} className='flex flex-wrap gap-x-5 gap-y-1 text-sm'>
        {choices.map((c) => (
          <label key={c.value} className='flex items-center gap-1.5'>
            <input
              type='radio'
              value={c.value}
              data-testid={`${id}-${c.value}`}
              {...form.register(name)}
            />
            {c.label}
          </label>
        ))}
      </div>
      {error && <FieldError data-testid={`${id}-error`}>{error}</FieldError>}
    </div>
  );
}

/** A longer fixed choice as a native select, so each option reads as a full sentence. */
export function SelectField<T extends string>({
  form,
  messageFor,
  name,
  label,
  hint,
  choices
}: SectionProps & {
  name: Path<DesignFormValues>;
  label: string;
  hint?: string;
  choices: Choice<T>[];
}) {
  const id = fieldId(name);
  return (
    <DesignField id={id} label={label} hint={hint} error={messageFor(name)}>
      <select
        id={id}
        data-testid={id}
        className='h-9 w-full rounded-md border border-input bg-background px-2 text-sm'
        {...form.register(name)}
      >
        {choices.map((c) => (
          <option key={c.value} value={c.value}>
            {c.label}
          </option>
        ))}
      </select>
    </DesignField>
  );
}

/** A titled group of fields. */
export function FormSection({
  id,
  title,
  description,
  children
}: {
  id: string;
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <fieldset
      className='flex flex-col gap-3 rounded-lg border p-4'
      data-testid={`experiment-design-section-${id}`}
    >
      <legend className='px-1 font-medium text-sm'>{title}</legend>
      {description && <p className='text-muted-foreground text-xs'>{description}</p>}
      {children}
    </fieldset>
  );
}

export const Grid = ({ children }: { children: ReactNode }) => (
  <div className='grid gap-3 sm:grid-cols-2'>{children}</div>
);
