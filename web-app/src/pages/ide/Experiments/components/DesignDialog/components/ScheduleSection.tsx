import { type Grain, periodWord } from "../../../grain";
import { timeZones, withCurrent } from "../lists";
import {
  ChoiceField,
  FormSection,
  Grid,
  type SectionProps,
  SelectField,
  TextField
} from "./fields";
import { GrainField, TimeDimensionField } from "./pickers";

const ZONES = timeZones().map((value) => ({ value, label: value }));

function grainHint(grain: Grain): string {
  const one = periodWord(grain);
  const many = periodWord(grain, { plural: true });
  if (grain === "hour")
    return "One reading per unit per hour. Every length below counts hours, and a switch can fall on any whole local hour. Needs a datetime calendar.";
  return `One reading per unit per ${one}. Every length below counts ${many}${
    grain === "day" ? "" : `, and every date must start a ${one}`
  }.`;
}

/** The design's shape, calendar, measurement windows and quality floors. */
export default function ScheduleSection(p: SectionProps) {
  const kind = p.form.watch("kind");
  const zone = p.form.watch("timezone") ?? "";
  const grain = p.form.watch("grain") ?? "day";
  const many = periodWord(grain, { plural: true });
  const Many = periodWord(grain, { plural: true, capital: true });
  return (
    <>
      <FormSection id='schedule' title='Schedule'>
        <ChoiceField
          {...p}
          name='kind'
          label='Design'
          choices={[
            { value: "waves", label: "Staggered waves" },
            { value: "switchback", label: "Switchback (the whole fleet alternates on and off)" }
          ]}
        />
        <Grid>
          <SelectField
            {...p}
            name='timezone'
            label='Time zone'
            choices={withCurrent(ZONES, zone)}
            hint='Every date in the design is a calendar date in this zone.'
          />
          <TimeDimensionField {...p} />
          <GrainField {...p} hint={grainHint(grain)} />
        </Grid>
        {kind === "waves" ? (
          <Grid>
            <TextField
              {...p}
              name='preDays'
              label={`${Many} measured before each switch`}
              type='number'
              min={1}
              step={1}
              placeholder='56'
            />
            <TextField
              {...p}
              name='postDays'
              label={`${Many} measured after each switch`}
              type='number'
              min={1}
              step={1}
              placeholder='56'
            />
            <TextField
              {...p}
              name='anticipationDays'
              label={`Anticipation buffer (${many})`}
              type='number'
              min={0}
              step={1}
              hint={`${Many} just before a switch left out, in case behaviour moves early.`}
            />
            <TextField
              {...p}
              name='washoutDays'
              label={`Washout (${many})`}
              type='number'
              min={0}
              step={1}
              hint={`${Many} just after a switch left out while the change settles.`}
            />
            <TextField
              {...p}
              name='carryoverDays'
              label={`Keep reading after it turns off (${many}, optional)`}
              type='number'
              min={1}
              step={1}
              hint='Measures the effect that lingers once the lever is off, such as ads seen before a purchase. Needs one switch date.'
            />
          </Grid>
        ) : (
          <Grid>
            <TextField
              {...p}
              name='periodDays'
              label={`Period length (${many})`}
              type='number'
              min={1}
              step={1}
              hint='Each on and each off period lasts this long.'
            />
            <TextField
              {...p}
              name='washoutDays'
              label={`Washout (${many})`}
              type='number'
              min={0}
              step={1}
              hint={`${Many} at the start of each period left out; shorter than the period.`}
            />
          </Grid>
        )}
      </FormSection>
      <FormSection
        id='quality'
        title='Quality floors'
        description='Below either floor the result is refused, or flagged as measuring the attempt.'
      >
        <Grid>
          <TextField
            {...p}
            name='coverageFloor'
            label={`${Many} with data in each window, at least (%)`}
            type='number'
            min={0}
            max={100}
            step='any'
          />
          <TextField
            {...p}
            name='complianceFloor'
            label='Switches that must land, at least (%)'
            type='number'
            min={0}
            max={100}
            step='any'
          />
        </Grid>
      </FormSection>
    </>
  );
}
