import ConditionField from "./ConditionField";
import { FormSection, Grid, type SectionProps, SelectField, TextField } from "./fields";
import { AutomationField } from "./pickers";

/**
 * How the treated units were chosen, and — for a change the platform switches —
 * the automations that switch it and how a switch is confirmed in the data. An
 * observed rollout already happened, so it has no lever.
 */
export default function LeverSection(p: SectionProps) {
  const observed = p.form.watch("assignment") === "observed";
  return (
    <FormSection
      id='lever'
      title='Lever'
      description='The change being tested, switched by automations.'
    >
      <Grid>
        <SelectField
          {...p}
          name='assignment'
          label='How the treated units are chosen'
          choices={[
            {
              value: "randomised",
              label: "Drawn at random by pricing, then switched by automations"
            },
            {
              value: "observed",
              label: "Already changed: list the units and dates, and analyse past data"
            }
          ]}
        />
      </Grid>
      {observed ? (
        <p className='text-muted-foreground text-sm' data-testid='experiment-design-observed-note'>
          Nothing is switched. Add the waves under Assignment below, with the units that changed and
          the date they changed; the units you leave out are the comparison. The result is labelled
          observational.
        </p>
      ) : (
        <LeverFields {...p} />
      )}
    </FormSection>
  );
}

function LeverFields(p: SectionProps) {
  return (
    <>
      <Grid>
        <AutomationField
          {...p}
          name='onVia'
          label='Turned on by (automation)'
          placeholder='fruit_summary'
        />
        <AutomationField
          {...p}
          name='offVia'
          label='Turned off by (automation)'
          placeholder='fruit_summary'
        />
        <SelectField
          {...p}
          name='per'
          label='How runs are made'
          choices={[
            { value: "unit", label: "One automation run for each unit and each switch" },
            {
              value: "wave",
              label: "One automation run for each wave and state, carrying all of the wave's units"
            }
          ]}
        />
        <SelectField
          {...p}
          name='onClose'
          label='At close'
          choices={[
            { value: "off", label: "Turned off when the experiment closes" },
            { value: "keep", label: "Left on when the experiment closes" }
          ]}
        />
      </Grid>
      <p className='text-sm'>A switch counts as confirmed when the data shows it:</p>
      <Grid>
        <ConditionField
          {...p}
          name='whenOn'
          label='Confirmed on when'
          hint='A measure compared with a number, for example orders.total_order_value above 0.'
        />
        <ConditionField {...p} name='whenOff' label='Confirmed off when' />
        <TextField
          {...p}
          name='graceDays'
          label='Days allowed to confirm a switch'
          type='number'
          min={0}
          step={1}
          placeholder='2'
        />
      </Grid>
    </>
  );
}
