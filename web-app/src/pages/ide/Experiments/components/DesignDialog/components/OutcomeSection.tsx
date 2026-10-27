import { withCurrent } from "../lists";
import {
  ChoiceField,
  FormSection,
  Grid,
  type SectionProps,
  SelectField,
  TextField
} from "./fields";
import MeasureFilterList from "./MeasureFilterList";
import { MeasureField } from "./pickers";

const ALPHAS = ["0.01", "0.05", "0.1", "0.2"].map((value) => ({ value, label: value }));
const POWERS = ["70", "80", "90", "95"].map((value) => ({ value, label: `${value}%` }));

/** The measure read, which way it should move, and how strict the test is. */
export default function OutcomeSection(p: SectionProps) {
  const alpha = p.form.watch("alpha") ?? "";
  const power = p.form.watch("power") ?? "";
  const target = p.form.watch("target") ?? "";
  const driver = p.form.watch("driver") ?? "";
  const sibling = p.form.watch("sibling") ?? "";
  return (
    <FormSection id='outcome' title='Outcome'>
      <MeasureField
        {...p}
        name='target'
        label='Outcome measure'
        placeholder='orders.total_order_value'
        hint='The measure from the semantic model that the experiment reads.'
      />
      <MeasureFilterList {...p} name='targetFilter' measure={target} title='Only rows where' />
      <MeasureField
        {...p}
        name='denominator'
        label='Divided by (optional)'
        placeholder='orders.total_units'
        hint='Makes the outcome a share per unit: the outcome measure over this one, summed over each window. Read from past data only.'
      />
      <ChoiceField
        {...p}
        name='direction'
        label='Expected direction'
        choices={[
          { value: "increase", label: "Expects an increase" },
          { value: "decrease", label: "Expects a decrease" }
        ]}
      />
      <Grid>
        <MeasureField
          {...p}
          name='driver'
          label='Driver (optional)'
          hint='A driver of the outcome in the World Model; its slope is set against what the experiment measures.'
        />
        <SelectField
          {...p}
          name='alpha'
          label='Significance level (α)'
          choices={withCurrent(ALPHAS, alpha)}
          hint='0.05 is a plain test. Above 0.05 the result is reported as directional.'
        />
        <SelectField
          {...p}
          name='power'
          label='Power'
          choices={withCurrent(POWERS, power)}
          hint='The chance of detecting the promised effect if it is real.'
        />
        <TextField
          {...p}
          name='family'
          label='Outcomes registered together'
          type='number'
          min={1}
          step={1}
          hint='More than 1 corrects the test for reading several outcomes at once.'
        />
      </Grid>
      {driver && (
        <MeasureFilterList
          {...p}
          name='driverFilter'
          measure={driver}
          title='Driver: only rows where'
        />
      )}
      <MeasureField
        {...p}
        name='sibling'
        label='Watch for cannibalisation of (optional)'
        placeholder='orders.other_order_value'
        hint='A sibling measure watched in case the change only moves value from it to the outcome.'
      />
      <MeasureFilterList {...p} name='siblingFilter' measure={sibling} title='Only rows where' />
    </FormSection>
  );
}
