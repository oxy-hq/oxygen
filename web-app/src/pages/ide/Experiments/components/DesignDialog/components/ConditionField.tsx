import { useEffect, useRef, useState } from "react";
import { type Path, useController } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { useMetricTree } from "@/hooks/api/useMetricTree";
import {
  type Condition,
  composeCondition,
  OPERATORS,
  type Operator,
  parseCondition
} from "../condition";
import type { DesignFormValues } from "../designForm";
import { measureGroups, measureMatch } from "../lists";
import { DesignField, fieldId, type SectionProps } from "./fields";
import { matchHint, PickerInput } from "./PickerField";

const BLANK: Condition = { measure: "", denominator: "", op: ">", value: "" };

/** How the condition is shown: built from parts, a stored one the builder cannot read, or edited as text. */
type Mode = "builder" | "readonly" | "text";

/**
 * A `{{view.measure}} <op> <number>` condition built from a measure picker, an
 * operator and a number, and stored as that same string. With `ratio` (a trigger
 * guardrail) the measure may be divided by a second one: `{{a}} / {{b}} <op> <number>`.
 * A stored value the server's grammar would not read as one stays as it is,
 * read-only, until the person chooses to edit it as text.
 */
export default function ConditionField({
  form,
  messageFor,
  name,
  label,
  hint,
  ratio = false
}: SectionProps & {
  name: Path<DesignFormValues>;
  label: string;
  hint?: string;
  ratio?: boolean;
}) {
  const { field } = useController({ control: form.control, name });
  const value = typeof field.value === "string" ? field.value : "";
  const { data: tree } = useMetricTree();
  const id = fieldId(name);
  const error = messageFor(name);

  const [mode, setMode] = useState<Mode>(() =>
    value === "" || parseCondition(value, { ratio }) ? "builder" : "readonly"
  );
  const [parts, setParts] = useState<Condition>(() => parseCondition(value, { ratio }) ?? BLANK);
  // The last value this field wrote itself: anything else arriving is an import or a reset.
  const written = useRef(value);

  useEffect(() => {
    if (value === written.current) return;
    written.current = value;
    const c = parseCondition(value, { ratio });
    if (value === "") setParts(BLANK);
    else if (c) setParts(c);
    setMode((m) => {
      if (m === "text") return m;
      return value === "" || c ? "builder" : "readonly";
    });
  }, [value, ratio]);

  const write = (next: string) => {
    written.current = next;
    field.onChange(next);
  };
  const update = (patch: Partial<Condition>) => {
    const next = { ...parts, ...patch };
    setParts(next);
    write(next.measure.trim() !== "" && next.value.trim() !== "" ? composeCondition(next) : "");
  };

  if (mode !== "builder") {
    const readable = value === "" || parseCondition(value, { ratio }) !== null;
    return (
      <DesignField id={id} label={label} hint={hint} error={error}>
        <div className='flex items-center gap-2'>
          <Input
            id={id}
            data-testid={id}
            ref={field.ref}
            value={value}
            readOnly={mode === "readonly"}
            aria-invalid={Boolean(error)}
            autoComplete='off'
            placeholder='{{orders.total_order_value}} > 0'
            onChange={(e) => write(e.target.value)}
            onBlur={field.onBlur}
          />
          {mode === "readonly" ? (
            <Button type='button' variant='outline' size='sm' onClick={() => setMode("text")}>
              Edit as text
            </Button>
          ) : (
            <Button
              type='button'
              variant='outline'
              size='sm'
              disabled={!readable}
              title={readable ? undefined : "The text is not a measure, an operator and a number"}
              onClick={() => {
                setParts(parseCondition(value, { ratio }) ?? BLANK);
                setMode("builder");
              }}
            >
              Use the builder
            </Button>
          )}
        </div>
        {mode === "readonly" && (
          <p className='text-muted-foreground text-xs'>
            The builder only writes a measure, an operator and a number. This condition is kept as
            it is.
          </p>
        )}
      </DesignField>
    );
  }

  return (
    <DesignField
      id={id}
      labelFor={`${id}-measure`}
      label={label}
      error={error}
      hint={matchHint(`${id}-measure`, measureMatch(parts.measure, tree?.nodes), hint)}
    >
      <div className='flex flex-wrap items-start gap-2'>
        <div className='min-w-48 flex-1'>
          <PickerInput
            id={`${id}-measure`}
            value={parts.measure}
            onChange={(measure) => update({ measure })}
            onBlur={field.onBlur}
            inputRef={field.ref}
            groups={measureGroups(tree?.nodes ?? [])}
            listLabel='Measures'
            placeholder='orders.total_order_value'
            invalid={Boolean(error)}
          />
        </div>
        {ratio && (
          <div className='min-w-48 flex-1'>
            <PickerInput
              id={`${id}-denominator`}
              value={parts.denominator ?? ""}
              onChange={(denominator) => update({ denominator })}
              groups={measureGroups(tree?.nodes ?? [])}
              listLabel='Measures'
              placeholder='divided by (optional)'
            />
          </div>
        )}
        <select
          id={`${id}-op`}
          data-testid={`${id}-op`}
          aria-label='Operator'
          className='h-9 rounded-md border border-input bg-background px-2 text-sm'
          value={parts.op}
          onChange={(e) => update({ op: e.target.value as Operator })}
        >
          {OPERATORS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
        <Input
          id={`${id}-value`}
          data-testid={`${id}-value`}
          aria-label='Number'
          type='number'
          step='any'
          className='w-28'
          value={parts.value}
          onChange={(e) => update({ value: e.target.value })}
        />
        <Button type='button' variant='ghost' size='sm' onClick={() => setMode("text")}>
          Edit as text
        </Button>
      </div>
      {/* The whole condition as text, for tools and people that type it in one go. */}
      <input
        id={id}
        data-testid={id}
        className='sr-only'
        tabIndex={-1}
        aria-hidden
        autoComplete='off'
        value={value}
        onChange={(e) => {
          const next = e.target.value;
          write(next);
          const c = parseCondition(next, { ratio });
          if (c) setParts(c);
          else if (next === "") setParts(BLANK);
        }}
      />
    </DesignField>
  );
}
