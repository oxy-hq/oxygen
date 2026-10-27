import { nameProblem } from "../toConfig";
import { FormSection, Grid, type SectionProps, TextField } from "./fields";
import OwnerField from "./OwnerField";
import { AssignByField, EntityField } from "./pickers";

/** What the experiment tests, who owns it, and what is randomised. */
export default function AboutSection(props: SectionProps & { nameLocked: boolean }) {
  const { nameLocked, ...p } = props;
  // The name is held to the identifier rule as it is typed, not only on save.
  const typed = (p.form.watch("experiment") ?? "").trim();
  const live = !nameLocked && typed !== "" ? nameProblem(typed) : undefined;
  const named = {
    ...p,
    messageFor: (name: string) => (name === "experiment" && live ? live : p.messageFor(name))
  };
  return (
    <FormSection id='about' title='What it tests'>
      <Grid>
        <TextField
          {...named}
          name='experiment'
          label='Experiment name'
          placeholder='customer_orders_pilot'
          readOnly={nameLocked}
          hint={
            nameLocked
              ? "The name cannot change once the experiment exists."
              : "A short identifier in letters, digits and underscores. It names the experiment everywhere."
          }
        />
        <OwnerField {...p} />
      </Grid>
      <TextField
        {...p}
        name='hypothesis'
        label='Hypothesis'
        multiline
        placeholder='A customer-level promo lifts order value per customer-day'
        hint='The effect you expect, in one sentence.'
      />
      <Grid>
        <EntityField {...p} />
        <AssignByField {...p} />
      </Grid>
    </FormSection>
  );
}
