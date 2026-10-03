import { Plus, Trash2, X } from "lucide-react";
import { useState } from "react";
import { Controller, useFormContext } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { CardTitle } from "@/components/ui/shadcn/card";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger
} from "@/components/ui/shadcn/collapsible";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { THINKING_OPTIONS, VENDOR_OPTIONS } from "./constants";
import type { AgenticFormData } from "./index";

/** `name` doubles as the trigger's id, so a `<Label htmlFor={name}>` names it; `label`
 *  replaces that name where the visible label alone is ambiguous. */
const ThinkingSelect = ({ name, label }: { name: string; label?: string }) => {
  const { control } = useFormContext<AgenticFormData>();
  return (
    <Controller
      name={name as never}
      control={control}
      render={({ field }) => (
        <div className='flex items-center gap-1'>
          <Select onValueChange={field.onChange} value={(field.value as string | undefined) ?? ""}>
            <SelectTrigger id={name} aria-label={label} className='flex-1'>
              <SelectValue placeholder='Select thinking mode' />
            </SelectTrigger>
            <SelectContent>
              {THINKING_OPTIONS.map((opt) => (
                <SelectItem className='cursor-pointer' key={opt.value} value={opt.value}>
                  {opt.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {field.value && (
            <Button
              type='button'
              variant='ghost'
              size='sm'
              className='px-2'
              onClick={() => field.onChange(undefined)}
              aria-label={`Clear ${(label ?? "thinking mode").toLowerCase()}`}
            >
              <X className='h-4 w-4' />
            </Button>
          )}
        </div>
      )}
    />
  );
};

export const LlmConfigForm: React.FC = () => {
  const { register, setValue, watch } = useFormContext<AgenticFormData>();
  const extendedThinking = watch("llm.extended_thinking");
  const ref = watch("llm.ref");
  const [showExtended, setShowExtended] = useState(!!extendedThinking);

  return (
    <div className='space-y-4'>
      <CardTitle>LLM Configuration</CardTitle>

      <div className='space-y-4 rounded-lg border p-4'>
        {/* ref */}
        <div className='space-y-2'>
          <Label htmlFor='llm.ref'>Provider Ref</Label>
          <Input id='llm.ref' placeholder='e.g., claude, openai' {...register("llm.ref")} />
          <p className='text-muted-foreground text-sm'>
            Named model reference from <code>config.yml</code>. Inherits vendor, API key, and base
            URL from that entry.
          </p>
        </div>

        {/* vendor — hidden when ref is set */}
        {!ref && (
          <div className='space-y-2'>
            <Label htmlFor='llm.vendor'>Vendor</Label>
            <Controller
              name='llm.vendor'
              render={({ field }) => (
                <Select onValueChange={field.onChange} value={field.value ?? ""}>
                  <SelectTrigger id='llm.vendor'>
                    <SelectValue placeholder='Select vendor (default: anthropic)' />
                  </SelectTrigger>
                  <SelectContent>
                    {VENDOR_OPTIONS.map((opt) => (
                      <SelectItem className='cursor-pointer' key={opt.value} value={opt.value}>
                        {opt.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            />
          </div>
        )}

        {/* model */}
        <div className='space-y-2'>
          <Label htmlFor='llm.model'>Model</Label>
          <Input
            id='llm.model'
            placeholder='e.g., claude-haiku-4-5, gpt-4o'
            {...register("llm.model")}
          />
          {ref && (
            <p className='text-muted-foreground text-sm'>
              Overrides the model resolved from <code>ref</code>.
            </p>
          )}
        </div>

        {/* max_tokens */}
        <div className='space-y-2'>
          <Label htmlFor='llm.max_tokens'>Max Tokens</Label>
          <Input
            id='llm.max_tokens'
            type='number'
            placeholder='Default: 4096'
            {...register("llm.max_tokens", { valueAsNumber: true })}
          />
        </div>

        {/* api_key */}
        <div className='space-y-2'>
          <Label htmlFor='llm.api_key'>API Key</Label>
          <Input
            id='llm.api_key'
            placeholder='e.g., $&#123;ANTHROPIC_API_KEY&#125;'
            {...register("llm.api_key")}
          />
          <p className='text-muted-foreground text-sm'>
            Supports $&#123;ENV_VAR&#125; interpolation. Falls back to environment variables when
            omitted.
          </p>
        </div>

        {/* base_url */}
        <div className='space-y-2'>
          <Label htmlFor='llm.base_url'>Base URL</Label>
          <Input
            id='llm.base_url'
            placeholder='e.g., http://localhost:11434/v1'
            {...register("llm.base_url")}
          />
          <p className='text-muted-foreground text-sm'>
            Override the API base URL. Required for OpenAI-compatible local servers.
          </p>
        </div>

        {/* thinking */}
        <div className='space-y-2'>
          <Label htmlFor='llm.thinking'>Thinking Mode</Label>
          <ThinkingSelect name='llm.thinking' />
          <p className='text-muted-foreground text-sm'>
            Applied to every pipeline state. Per-state overrides take precedence.
          </p>
        </div>

        {/* extended_thinking */}
        {!showExtended ? (
          <Button type='button' variant='outline' size='sm' onClick={() => setShowExtended(true)}>
            <Plus />
            Add Extended Thinking
          </Button>
        ) : (
          <Collapsible defaultOpen>
            {/* The remove control sits beside the trigger, not in it: a button inside a
                button is invalid. */}
            <div className='flex w-full items-center justify-between'>
              <CollapsibleTrigger className='flex flex-1 items-center self-stretch'>
                <span className='font-medium text-sm'>Extended Thinking</span>
              </CollapsibleTrigger>
              <Button
                type='button'
                variant='ghost'
                size='sm'
                aria-label='Remove extended thinking'
                onClick={() => {
                  setValue("llm.extended_thinking", undefined, { shouldDirty: true });
                  setShowExtended(false);
                }}
              >
                <Trash2 className='h-4 w-4' />
              </Button>
            </div>
            <CollapsibleContent className='mt-3 space-y-3 pl-4'>
              <p className='text-muted-foreground text-sm'>
                Alternative model + thinking config activated via the UI toggle for deeper
                reasoning.
              </p>
              <div className='space-y-2'>
                <Label htmlFor='llm.extended_thinking.model'>Model</Label>
                <Input
                  id='llm.extended_thinking.model'
                  aria-label='Extended thinking model'
                  placeholder='e.g., claude-opus-4-6'
                  {...register("llm.extended_thinking.model")}
                />
              </div>
              <div className='space-y-2'>
                <Label htmlFor='llm.extended_thinking.thinking'>Thinking Mode</Label>
                <ThinkingSelect
                  name='llm.extended_thinking.thinking'
                  label='Extended thinking mode'
                />
              </div>
            </CollapsibleContent>
          </Collapsible>
        )}
      </div>
    </div>
  );
};
