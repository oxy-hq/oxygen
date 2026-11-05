import { Trash2 } from "lucide-react";
import { useId, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Select, SelectContent, SelectItem, SelectTrigger } from "@/components/ui/shadcn/select";
import { cn } from "@/libs/shadcn/utils";
import { isUuid, type OrgOption, resolveOrgId } from "./rules";

type Kind = "org" | "user";

function VariantSelect({
  value,
  variants,
  onChange,
  testId
}: {
  value: string;
  variants: string[];
  onChange: (variant: string) => void;
  testId: string;
}) {
  const options = variants.includes(value) ? variants : [...variants, value];
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger size='sm' className='w-32 text-xs' data-testid={testId}>
        <span className='truncate font-mono'>{value}</span>
      </SelectTrigger>
      <SelectContent>
        {options.map((v) => (
          <SelectItem key={v} value={v} className='font-mono text-xs'>
            {v}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/**
 * Per-org or per-user overrides: one row each (name or uuid, variant, remove) and an
 * add row. Orgs are added by name (from the admin list) or pasted uuid; users by uuid.
 */
export function OverrideList({
  kind,
  entries,
  variants,
  orgs,
  onChange
}: {
  kind: Kind;
  entries: Record<string, string>;
  variants: string[];
  orgs: OrgOption[];
  onChange: (next: Record<string, string>) => void;
}) {
  const listId = useId();
  const [text, setText] = useState("");
  const [variant, setVariant] = useState(variants[1] ?? variants[0]);
  const nameOf = (id: string) => (kind === "org" ? orgs.find((o) => o.id === id)?.name : undefined);
  const resolved = kind === "org" ? resolveOrgId(text, orgs) : isUuid(text) ? text.trim() : null;

  const add = () => {
    if (!resolved) return;
    onChange({ ...entries, [resolved]: variant });
    setText("");
  };
  const remove = (id: string) => {
    const { [id]: _removed, ...rest } = entries;
    onChange(rest);
  };

  return (
    <div className='space-y-2'>
      {Object.entries(entries).map(([id, v]) => (
        <div key={id} className='flex items-center gap-2' data-testid={`feature-flag-${kind}-row`}>
          <div className='min-w-0 flex-1'>
            <div className='truncate font-medium text-xs'>{nameOf(id) ?? id}</div>
            {nameOf(id) && (
              <div className='truncate font-mono text-muted-foreground text-xs'>{id}</div>
            )}
          </div>
          <VariantSelect
            value={v}
            variants={variants}
            onChange={(next) => onChange({ ...entries, [id]: next })}
            testId={`feature-flag-${kind}-variant-${id}`}
          />
          <Button
            type='button'
            variant='ghost'
            size='icon'
            className='size-8 text-muted-foreground hover:text-destructive'
            aria-label={`Remove ${nameOf(id) ?? id}`}
            data-testid={`feature-flag-${kind}-remove-${id}`}
            onClick={() => remove(id)}
          >
            <Trash2 className='size-3.5' />
          </Button>
        </div>
      ))}
      <div className='flex items-center gap-2'>
        <Input
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              add();
            }
          }}
          list={kind === "org" ? listId : undefined}
          placeholder={kind === "org" ? "Org name or uuid" : "User uuid"}
          className={cn("h-8 flex-1 text-xs", kind === "user" && "font-mono")}
          data-testid={`feature-flag-${kind}-input`}
        />
        {kind === "org" && (
          <datalist id={listId}>
            {orgs.map((o) => (
              <option key={o.id} value={o.name} label={o.id} />
            ))}
          </datalist>
        )}
        <VariantSelect
          value={variant}
          variants={variants}
          onChange={setVariant}
          testId={`feature-flag-${kind}-new-variant`}
        />
        <Button
          type='button'
          size='sm'
          variant='outline'
          className='text-xs'
          disabled={!resolved}
          onClick={add}
          data-testid={`feature-flag-${kind}-add`}
        >
          Add
        </Button>
      </div>
    </div>
  );
}
