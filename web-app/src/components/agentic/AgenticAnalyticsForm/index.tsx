import { useEffect } from "react";
import { FormProvider, useForm } from "react-hook-form";
import { Label } from "@/components/ui/shadcn/label";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { cleanObject } from "@/utils/formDataCleaner";
import { ContextGlobForm } from "./ContextGlobForm";
import { DatabasesForm } from "./DatabasesForm";
import { LlmConfigForm } from "./LlmConfigForm";
import { SemanticEngineForm } from "./SemanticEngineForm";
import { StateOverridesForm } from "./StateOverridesForm";
import { ValidationForm } from "./ValidationForm";
import { toYamlValidation } from "./ValidationForm/utils";

// ─── Types ───────────────────────────────────────────────────────────────────

interface LlmExtendedThinkingData {
  model?: string;
  thinking?: string;
}

interface LlmConfigData {
  ref?: string;
  vendor?: string;
  api_key?: string;
  base_url?: string;
  model?: string;
  max_tokens?: number;
  thinking?: string;
  extended_thinking?: LlmExtendedThinkingData;
}

interface StateConfigData {
  instructions?: string;
  model?: string;
  max_retries?: number;
  thinking?: string;
}

/** One rule entry. Keys besides `name` and `enabled` are that rule's parameters. */
export interface ValidationRuleData {
  name?: string;
  enabled?: boolean;
  // sql_syntax
  dialect?: string;
  // outlier_detection
  threshold_sigma?: number;
  min_rows?: number;
  // null_ratio_check
  threshold?: number;
  // duplicate_row_check
  max_duplicate_ratio?: number;
  // freshness_check
  threshold_days?: number;
}

export interface ValidationConfigData {
  rules?: {
    specified?: ValidationRuleData[];
    solvable?: ValidationRuleData[];
    solved?: ValidationRuleData[];
  };
}

interface SemanticEngineData {
  vendor?: string;
  base_url?: string;
  api_token?: string;
  client_id?: string;
  client_secret?: string;
}

/** Form-internal representation. String arrays are stored as {value: string}[]
 *  so react-hook-form's useFieldArray can manage them. */
export interface AgenticFormData {
  instructions?: string;
  databases?: { value: string }[];
  llm?: LlmConfigData;
  context?: { value: string }[];
  thinking?: string;
  states?: {
    clarifying?: StateConfigData;
    specifying?: StateConfigData;
    solving?: StateConfigData;
    executing?: StateConfigData;
    interpreting?: StateConfigData;
    diagnosing?: StateConfigData;
  };
  validation?: ValidationConfigData;
  semantic_engine?: SemanticEngineData;
}

/** The shape that maps 1:1 to the YAML file (string arrays, not object arrays). */
export interface AgenticYamlData {
  instructions?: string;
  databases?: string[];
  llm?: LlmConfigData;
  context?: string[];
  thinking?: string;
  states?: AgenticFormData["states"];
  validation?: ValidationConfigData;
  semantic_engine?: SemanticEngineData;
}

// ─── Converters ──────────────────────────────────────────────────────────────

/** Convert YAML data → form data (wrap string arrays). */
export const yamlToForm = (yaml: AgenticYamlData): AgenticFormData => ({
  ...yaml,
  databases: (yaml.databases ?? []).map((v) => ({ value: v })),
  context: (yaml.context ?? []).map((v) => ({ value: v }))
});

/** Convert form data → YAML data (unwrap string arrays, strip empties). */
const formToYaml = (form: AgenticFormData): AgenticYamlData => {
  const raw: AgenticYamlData = {
    ...form,
    databases: form.databases?.map((d) => d.value).filter(Boolean),
    context: form.context?.map((c) => c.value).filter(Boolean)
  };
  const yaml = (cleanObject(raw as Record<string, unknown>) as AgenticYamlData | null) ?? {};
  // An empty `validation:` section runs no rules and an absent one runs them
  // all, so it must survive the empty-stripping above.
  if (form.validation != null) yaml.validation = toYamlValidation(form.validation);
  return yaml;
};

// ─── Component ───────────────────────────────────────────────────────────────

interface AgenticAnalyticsFormProps {
  data?: AgenticFormData;
  onChange?: (data: AgenticYamlData) => void;
}

export const AgenticAnalyticsForm: React.FC<AgenticAnalyticsFormProps> = ({ data, onChange }) => {
  const methods = useForm<AgenticFormData>({
    defaultValues: data ?? {},
    mode: "onBlur"
  });

  const { subscribe, register } = methods;

  useEffect(() => {
    const unsub = subscribe({
      formState: { values: true, isDirty: true },
      callback: ({ values, isDirty }) => {
        if (isDirty) {
          onChange?.(formToYaml(values as AgenticFormData));
        }
      }
    });
    return () => unsub();
  }, [subscribe, onChange]);

  return (
    <FormProvider {...methods}>
      <div className='flex min-h-0 flex-1 flex-col'>
        <div className='customScrollbar flex-1 overflow-auto p-4'>
          <form id='agentic-analytics-form' className='space-y-8'>
            {/* Instructions */}
            <div className='space-y-2'>
              <Label htmlFor='instructions'>Instructions</Label>
              <Textarea
                id='instructions'
                placeholder='Global instructions injected into every LLM call. Supports Jinja2 templating.'
                rows={4}
                {...register("instructions")}
              />
              <p className='text-muted-foreground text-sm'>Applied to all pipeline states.</p>
            </div>

            <LlmConfigForm />
            <ContextGlobForm />
            <DatabasesForm />
            <StateOverridesForm />
            <ValidationForm />
            <SemanticEngineForm />
          </form>
        </div>
      </div>
    </FormProvider>
  );
};
