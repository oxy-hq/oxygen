import type { LucideIcon } from "lucide-react";
import { type Ref, useRef, useState } from "react";
import { type Path, useController } from "react-hook-form";
import { Input } from "@/components/ui/shadcn/input";
import { Popover, PopoverAnchor, PopoverContent } from "@/components/ui/shadcn/popover";
import { cn } from "@/libs/shadcn/utils";
import { type Tone, ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import type { DesignFormValues } from "../designForm";
import { DesignField, fieldId, type SectionProps } from "./fields";

/** The verdict on a picker's current value. */
export interface Match {
  tone: Tone;
  text: string;
}

export interface PickerOption {
  value: string;
  label: string;
  detail?: string;
  /** What typing filters on; defaults to the value, label and detail. */
  search?: string;
}

export interface PickerGroup {
  title?: string;
  options: PickerOption[];
}

function Group({
  group,
  current,
  onPick
}: {
  group: PickerGroup;
  current: string;
  onPick: (v: string) => void;
}) {
  return (
    <div className='py-1'>
      {group.title && (
        <div className='px-2 py-1 font-medium text-muted-foreground text-xs'>{group.title}</div>
      )}
      {group.options.map((o) => (
        <div
          key={o.value}
          role='option'
          tabIndex={-1}
          aria-selected={o.value === current}
          data-value={o.value}
          className={cn(
            "flex cursor-pointer items-baseline justify-between gap-3 rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground",
            o.value === current && "bg-accent text-accent-foreground"
          )}
          // Keep focus in the input: a blur before the click would close the list first.
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onPick(o.value)}
          onKeyDown={(e) => e.key === "Enter" && onPick(o.value)}
        >
          <span className='truncate'>{o.label}</span>
          {o.detail && (
            <span className='ml-3 min-w-0 max-w-1/2 truncate text-muted-foreground text-xs'>
              {o.detail}
            </span>
          )}
        </div>
      ))}
    </div>
  );
}

/** The groups that match what is typed. A value that is itself an option shows the whole list. */
export function filterGroups(groups: PickerGroup[], typed: string): PickerGroup[] {
  const q = typed.trim().toLowerCase();
  const isOption = groups.some((g) => g.options.some((o) => o.value === typed));
  if (!q || isOption) return groups.filter((g) => g.options.length > 0);
  return groups
    .map((g) => ({
      ...g,
      options: g.options.filter((o) =>
        (o.search ?? `${o.value} ${o.label} ${o.detail ?? ""}`).toLowerCase().includes(q)
      )
    }))
    .filter((g) => g.options.length > 0);
}

export interface PickerInputProps {
  id: string;
  /** The testid when it is not the id. */
  testId?: string;
  value: string;
  onChange: (value: string) => void;
  onBlur?: () => void;
  inputRef?: Ref<HTMLInputElement>;
  groups: PickerGroup[];
  listLabel: string;
  placeholder?: string;
  icon?: LucideIcon;
  invalid?: boolean;
  readOnly?: boolean;
}

/**
 * A text input with a styled list under it, filtered by what is typed. The list
 * only suggests: any text still goes through, so a stored value that is no longer
 * an option stays editable (the caller badges it).
 */
export function PickerInput(props: PickerInputProps) {
  const { id, value, onChange, groups, listLabel, icon: Icon, invalid, readOnly } = props;
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLDivElement>(null);
  const shown = filterGroups(groups, value);
  const hasOptions = !readOnly && shown.length > 0;

  const pick = (v: string) => {
    onChange(v);
    setOpen(false);
  };

  return (
    <Popover open={open && hasOptions} onOpenChange={setOpen}>
      <PopoverAnchor asChild>
        <div ref={anchor} className='relative'>
          {Icon && (
            <Icon
              aria-hidden
              className='pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground'
            />
          )}
          <Input
            id={id}
            data-testid={props.testId ?? id}
            ref={props.inputRef}
            className={Icon ? "pl-8" : undefined}
            aria-invalid={invalid}
            placeholder={props.placeholder}
            readOnly={readOnly}
            value={value}
            // Not an identity field: keep browser and password-manager autofill off it.
            autoComplete='off'
            data-bwignore='true'
            data-1p-ignore='true'
            data-lpignore='true'
            role='combobox'
            aria-expanded={open && hasOptions}
            aria-controls={`${id}-options`}
            onChange={(e) => {
              onChange(e.target.value);
              setOpen(true);
            }}
            onFocus={() => setOpen(true)}
            onBlur={() => {
              props.onBlur?.();
              setOpen(false);
            }}
            onKeyDown={(e) => {
              // Escape closes the open list and nothing else; the dialog gets it once the list is shut.
              if (e.key === "Escape" && open && hasOptions) {
                e.preventDefault();
                e.stopPropagation();
                e.nativeEvent.stopImmediatePropagation();
                setOpen(false);
              }
            }}
          />
        </div>
      </PopoverAnchor>
      <PopoverContent
        align='start'
        className='max-h-72 w-[var(--radix-popover-trigger-width)] overflow-y-auto p-1'
        onOpenAutoFocus={(e) => e.preventDefault()}
        onEscapeKeyDown={(e) => {
          e.preventDefault();
          setOpen(false);
        }}
        // The input is the anchor, not a trigger: clicking it must not count as outside.
        onInteractOutside={(e) => {
          if (anchor.current?.contains(e.target as Node)) e.preventDefault();
        }}
        onCloseAutoFocus={(e) => e.preventDefault()}
      >
        <div id={`${id}-options`} role='listbox' aria-label={listLabel}>
          {shown.map((g) => (
            <Group key={g.title ?? "all"} group={g} current={value} onPick={pick} />
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** The verdict as the field's line under the input, or the hint while there is none. */
export function matchHint(id: string, match: Match | null | undefined, hint?: React.ReactNode) {
  if (!match) return hint;
  return (
    <span data-testid={`${id}-match`} data-tone={match.tone}>
      <ToneBadge tone={match.tone}>{match.text}</ToneBadge>
    </span>
  );
}

export type PickerFieldProps = SectionProps & {
  name: Path<DesignFormValues>;
  label: string;
  hint?: React.ReactNode;
  placeholder?: string;
  icon?: LucideIcon;
  groups: PickerGroup[];
  /** The accessible name of the list. */
  listLabel: string;
  match?: Match | null;
  className?: string;
};

/** A picker bound to one form field; `fieldId(name)` is the input's id and testid. */
export function PickerField({
  form,
  messageFor,
  name,
  label,
  hint,
  placeholder,
  icon,
  groups,
  listLabel,
  match,
  className
}: PickerFieldProps) {
  const { field } = useController({ control: form.control, name });
  const id = fieldId(name);
  const error = messageFor(name);
  return (
    <DesignField
      id={id}
      label={label}
      error={error}
      hint={matchHint(id, match, hint)}
      className={className}
    >
      <PickerInput
        id={id}
        value={typeof field.value === "string" ? field.value : ""}
        onChange={field.onChange}
        onBlur={field.onBlur}
        inputRef={field.ref}
        groups={groups}
        listLabel={listLabel}
        placeholder={placeholder}
        icon={icon}
        invalid={Boolean(error)}
      />
    </DesignField>
  );
}
