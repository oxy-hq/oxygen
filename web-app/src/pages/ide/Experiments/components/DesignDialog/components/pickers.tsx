import type { Path } from "react-hook-form";
import useAutomations from "@/hooks/api/automations/useAutomations";
import { useMetricTree, useTimeDimensions } from "@/hooks/api/useMetricTree";
import { useWorldModel } from "@/hooks/api/useWorldModel";
import { grainChoices } from "@/pages/ide/Experiments/grain";
import type { DesignFormValues } from "../designForm";
import {
  automationGroups,
  automationMatch,
  dimensionGroups,
  dimensionMatch,
  entityGroups,
  entityMatch,
  measureGroups,
  measureMatch,
  timeDimensionGroups,
  timeDimensionMatch
} from "../lists";
import { type SectionProps, SelectField } from "./fields";
import { PickerField } from "./PickerField";

/** The pickers whose options come from the workspace: its semantic model and automations. */

type Named = SectionProps & {
  name: Path<DesignFormValues>;
  label: string;
  hint?: string;
  placeholder?: string;
};

/** The entity that is randomised: one of the semantic model's entities. */
export function EntityField(p: SectionProps) {
  const { data } = useWorldModel();
  const value = p.form.watch("forEach") ?? "";
  return (
    <PickerField
      {...p}
      name='forEach'
      label='Randomises each'
      placeholder='customer'
      hint='The entity whose instances are switched on and off, such as customer or store.'
      groups={entityGroups(data?.entities ?? [])}
      listLabel='Entities'
      match={entityMatch(value, data?.entities)}
    />
  );
}

/** A dimension of the chosen entity. */
export function AssignByField(p: SectionProps) {
  const { data } = useWorldModel();
  const entity = p.form.watch("forEach") ?? "";
  const value = p.form.watch("assignBy") ?? "";
  return (
    <PickerField
      {...p}
      name='assignBy'
      label='Randomise groups by (optional)'
      placeholder='category'
      hint='A dimension of that entity: its values, not single units, are randomised. Usually set by pricing.'
      groups={dimensionGroups(entity, data?.entities)}
      listLabel='Dimensions'
      match={dimensionMatch(value, entity, data?.entities)}
    />
  );
}

/** A measure of the semantic model, searchable, grouped by view. */
export function MeasureField(p: Named) {
  const { data } = useMetricTree();
  const value = p.form.watch(p.name);
  return (
    <PickerField
      {...p}
      groups={measureGroups(data?.nodes ?? [])}
      listLabel='Measures'
      match={measureMatch(typeof value === "string" ? value : "", data?.nodes)}
    />
  );
}

/** An automation of the workspace, by name. */
export function AutomationField(p: Named) {
  const { data } = useAutomations();
  const value = p.form.watch(p.name);
  return (
    <PickerField
      {...p}
      groups={automationGroups(data ?? [])}
      listLabel='Automations'
      match={automationMatch(typeof value === "string" ? value : "", data)}
    />
  );
}

/** A date dimension of the outcome's view. */
export function TimeDimensionField(p: SectionProps) {
  const { data } = useTimeDimensions();
  const target = p.form.watch("target") ?? "";
  const value = p.form.watch("timeDimension") ?? "";
  return (
    <PickerField
      {...p}
      name='timeDimension'
      label='Calendar date dimension (optional)'
      placeholder='orders.order_date'
      hint="The date the outcome's calendar runs on. Needed when the outcome's view has several dates."
      groups={timeDimensionGroups(target, data?.by_view ?? {})}
      listLabel='Date dimensions'
      match={timeDimensionMatch(value, target, data?.by_view)}
    />
  );
}

/** The grain picker: hour is listed only when the outcome's calendar is a datetime. */
export function GrainField(p: SectionProps & { hint: string }) {
  const { data } = useTimeDimensions();
  const target = p.form.watch("target") ?? "";
  const timeDimension = p.form.watch("timeDimension") ?? "";
  const current = p.form.watch("grain");
  return (
    <SelectField
      {...p}
      name='grain'
      label='Measure the outcome per'
      choices={grainChoices(target, timeDimension, data, current)}
    />
  );
}
