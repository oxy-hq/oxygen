import { cleanObject } from "@/utils/formDataCleaner";
import type { ValidationConfigData, ValidationRuleData } from "../index";
import { RULES_BY_STAGE, VALIDATION_STAGES, type ValidationStage } from "./constants";

type StageRules = Partial<Record<ValidationStage, ValidationRuleData[]>>;

/** The rules that run while the file has no `validation:` section, as an explicit list. */
export const defaultValidation = (): ValidationConfigData => {
  const rules: StageRules = {};
  for (const { stage } of VALIDATION_STAGES) {
    rules[stage] = RULES_BY_STAGE[stage]
      .filter((rule) => !rule.offByDefault)
      .map((rule) => ({ name: rule.value, enabled: true }));
  }
  return { rules };
};

/**
 * Serialize a `validation:` section that is present in the form.
 *
 * The backend reads the section as the complete rule list: absent, every
 * built-in rule runs; present, only the listed ones do, so an empty section
 * runs none. `cleanObject` cannot tell those apart and drops an empty section,
 * so the caller keeps the section and this cleans inside it. Within a present
 * section an empty stage and a missing one are the same, so empty stages go.
 * A rule with no name yet (just added) is left out: the backend rejects it.
 */
export const toYamlValidation = (validation: ValidationConfigData): ValidationConfigData => {
  const rules: StageRules = {};
  for (const { stage } of VALIDATION_STAGES) {
    const entries = (validation.rules?.[stage] ?? []).flatMap((rule) => {
      const cleaned = cleanObject({ ...rule });
      return cleaned?.name ? [cleaned as ValidationRuleData] : [];
    });
    if (entries.length > 0) rules[stage] = entries;
  }
  return { ...validation, rules };
};
