import { Plus, Trash2 } from "lucide-react";
import { useId, useState } from "react";
import { Controller, useFormContext } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { CardTitle } from "@/components/ui/shadcn/card";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { SEMANTIC_ENGINE_VENDORS } from "./constants";
import type { AgenticFormData } from "./index";

const RequiredMark = () => (
  <span aria-hidden='true' className='text-destructive'>
    *
  </span>
);

export const SemanticEngineForm: React.FC = () => {
  const { control, register, setValue, watch } = useFormContext<AgenticFormData>();
  const id = useId();
  const [showSection, setShowSection] = useState(watch("semantic_engine") != null);
  const vendor = watch("semantic_engine.vendor");
  const isLooker = vendor === "looker";
  const isCube = vendor === "cube";

  return (
    <section aria-labelledby={`${id}-title`} className='space-y-4'>
      <div className='flex items-center justify-between gap-2'>
        <CardTitle id={`${id}-title`}>Semantic Engine</CardTitle>
        {showSection && (
          <Button
            type='button'
            variant='ghost'
            size='sm'
            aria-label='Remove semantic engine'
            onClick={() => {
              setValue("semantic_engine", undefined, { shouldDirty: true });
              setShowSection(false);
            }}
          >
            <Trash2 className='h-4 w-4' />
          </Button>
        )}
      </div>

      {!showSection ? (
        <div className='space-y-2'>
          <p className='text-muted-foreground text-sm'>
            Optional vendor engine (Cube, Looker) the agent can delegate queries to. Without one,
            queries go through the semantic model and SQL generation.
          </p>
          <Button type='button' variant='outline' size='sm' onClick={() => setShowSection(true)}>
            <Plus />
            Add Semantic Engine
          </Button>
        </div>
      ) : (
        <div className='space-y-4 rounded-lg border p-4'>
          <div className='space-y-2'>
            <Label htmlFor={`${id}-vendor`}>
              Vendor <RequiredMark />
            </Label>
            <Controller
              name='semantic_engine.vendor'
              control={control}
              render={({ field }) => (
                <Select required onValueChange={field.onChange} value={field.value ?? ""}>
                  <SelectTrigger id={`${id}-vendor`}>
                    <SelectValue placeholder='Select vendor' />
                  </SelectTrigger>
                  <SelectContent>
                    {SEMANTIC_ENGINE_VENDORS.map((opt) => (
                      <SelectItem className='cursor-pointer' key={opt.value} value={opt.value}>
                        {opt.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            />
          </div>

          <div className='space-y-2'>
            <Label htmlFor={`${id}-base-url`}>
              Base URL <RequiredMark />
            </Label>
            <Input
              id={`${id}-base-url`}
              required
              placeholder={
                isLooker ? "e.g., https://myco.looker.com" : "e.g., https://cube.example.com"
              }
              {...register("semantic_engine.base_url")}
            />
          </div>

          {(isCube || !vendor) && (
            <div className='space-y-2'>
              <Label htmlFor={`${id}-api-token`}>API Token</Label>
              <Input
                id={`${id}-api-token`}
                placeholder='e.g., $&#123;CUBE_API_TOKEN&#125;'
                {...register("semantic_engine.api_token")}
              />
              <p className='text-muted-foreground text-sm'>
                Required for Cube. Supports $&#123;ENV_VAR&#125; interpolation.
              </p>
            </div>
          )}

          {(isLooker || !vendor) && (
            <>
              <div className='space-y-2'>
                <Label htmlFor={`${id}-client-id`}>Client ID</Label>
                <Input
                  id={`${id}-client-id`}
                  placeholder='e.g., $&#123;LOOKER_CLIENT_ID&#125;'
                  {...register("semantic_engine.client_id")}
                />
              </div>
              <div className='space-y-2'>
                <Label htmlFor={`${id}-client-secret`}>Client Secret</Label>
                <Input
                  id={`${id}-client-secret`}
                  placeholder='e.g., $&#123;LOOKER_CLIENT_SECRET&#125;'
                  {...register("semantic_engine.client_secret")}
                />
                <p className='text-muted-foreground text-sm'>
                  Required for Looker, with Client ID. Supports $&#123;ENV_VAR&#125; interpolation.
                </p>
              </div>
            </>
          )}
        </div>
      )}
    </section>
  );
};
