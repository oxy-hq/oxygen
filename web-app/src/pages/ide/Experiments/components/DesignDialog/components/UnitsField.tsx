import { X } from "lucide-react";
import { useState } from "react";
import { type Path, useController } from "react-hook-form";
import { useWmInstances } from "@/hooks/api/useWorldModel";
import type { DesignFormValues } from "../designForm";
import { DesignField, fieldId, type SectionProps, TextField } from "./fields";
import { PickerInput } from "./PickerField";

/** The names a unit list holds, split the way `toConfig` splits them. */
export const splitUnits = (raw: string) =>
  raw
    .split(/[\n,]/)
    .map((u) => u.trim())
    .filter((u) => u !== "");

/**
 * A list of the randomised entity's instances (units, or groups under
 * `assign_by`): chips for those chosen and a searchable list of the rest. Stored
 * as the comma list the form has always held. Without an entity, or when its
 * instances cannot be listed, it is the plain text box it was.
 */
export default function UnitsField(
  props: SectionProps & { name: Path<DesignFormValues>; label: string; hint: string }
) {
  const { form, messageFor, name, label, hint } = props;
  const entity = (form.watch("forEach") ?? "").trim();
  const assignBy = (form.watch("assignBy") ?? "").trim();
  const [search, setSearch] = useState("");
  const { field } = useController({ control: form.control, name });
  // The unfiltered list says whether instances can be listed at all; it stays put while a search loads.
  const probe = useWmInstances(entity || null, "");
  const searched = useWmInstances(entity || null, search);
  const data = searched.data ?? probe.data;
  const id = fieldId(name);
  const raw = typeof field.value === "string" ? field.value : "";
  const chosen = splitUnits(raw);

  // Under `assign_by` the names are groups of the entity, which this list does not know.
  if (!entity || assignBy || !probe.data) {
    return (
      <TextField
        form={form}
        messageFor={messageFor}
        name={name}
        label={label}
        multiline
        hint={hint}
      />
    );
  }

  const write = (units: string[]) => field.onChange(units.join(", "));
  const q = search.trim().toLowerCase();
  const options = (data?.items ?? [])
    .filter((i) => !chosen.includes(i.key))
    .filter((i) => !q || `${i.key} ${i.display}`.toLowerCase().includes(q))
    .map((i) => ({ value: i.key, label: i.key, detail: i.display !== i.key ? i.display : "" }));

  return (
    <DesignField
      id={id}
      labelFor={`${id}-search`}
      label={label}
      error={messageFor(name)}
      hint={
        data?.has_more
          ? "More match than are listed: keep typing to narrow the list."
          : `Instances of ${entity}. ${hint}`
      }
    >
      <div className='flex flex-col gap-2'>
        {chosen.length > 0 && (
          <ul className='flex flex-wrap gap-1.5' data-testid={`${id}-chips`}>
            {chosen.map((u) => (
              <li
                key={u}
                className='flex items-center gap-1 rounded-md border bg-muted px-2 py-0.5 text-xs'
              >
                {u}
                <button
                  type='button'
                  aria-label={`Remove ${u}`}
                  className='text-muted-foreground hover:text-foreground'
                  onClick={() => write(chosen.filter((x) => x !== u))}
                >
                  <X aria-hidden className='size-3' />
                </button>
              </li>
            ))}
          </ul>
        )}
        <PickerInput
          id={`${id}-search`}
          value={search}
          onChange={(v) => {
            if (options.some((o) => o.value === v)) {
              write([...chosen, v]);
              setSearch("");
            } else {
              setSearch(v);
            }
          }}
          onBlur={field.onBlur}
          inputRef={field.ref}
          groups={[{ options }]}
          listLabel={`${label} suggestions`}
          placeholder={`Search ${entity} instances`}
        />
        {/* The whole list as text, for tools and people that type it in one go. */}
        <input
          id={id}
          data-testid={id}
          className='sr-only'
          tabIndex={-1}
          aria-hidden
          autoComplete='off'
          value={raw}
          onChange={(e) => field.onChange(e.target.value)}
        />
      </div>
    </DesignField>
  );
}
