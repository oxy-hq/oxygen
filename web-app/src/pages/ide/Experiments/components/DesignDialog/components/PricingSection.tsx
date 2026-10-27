import { useFieldArray } from "react-hook-form";
import { switchInputType } from "@/pages/ide/Experiments/grain";
import { newPeriod, newStratum, newWave } from "../designForm";
import { Grid, type SectionProps, SelectField, TextField } from "./fields";
import RowList from "./RowList";
import UnitsField from "./UnitsField";

/**
 * What pricing writes when a priced design is used: the assignment, its seed and
 * the promised effect. Collapsed unless the design already carries some of it;
 * editing by hand is for correcting a design, not the usual path.
 */
export default function PricingSection(p: SectionProps) {
  const switchType = switchInputType(p.form.watch("grain") ?? "day");
  const { control, watch, getValues } = p.form;
  const waves = useFieldArray({ control, name: "waves" });
  const strata = useFieldArray({ control, name: "strata" });
  const periods = useFieldArray({ control, name: "periods" });
  const kind = watch("kind");
  const observed = watch("assignment") === "observed";
  const v = getValues();
  const filled =
    v.waves.length + v.strata.length + v.periods.length > 0 ||
    v.seed !== "" ||
    v.mdeAtProposal !== "" ||
    v.priorValue !== "";
  return (
    <details
      open={filled || observed}
      className='rounded-lg border p-4'
      data-testid='experiment-design-section-pricing'
    >
      <summary className='cursor-pointer font-medium text-sm'>
        {observed ? "Assignment: what changed, and when" : "Set by pricing"}
      </summary>
      <div className='mt-3 flex flex-col gap-4'>
        <p className='text-muted-foreground text-xs'>
          {observed
            ? "One wave per date the change went live, with the units it reached. No seed is needed; pricing is optional."
            : '"Use this design" on a priced candidate fills these in. Leave them empty for a draft; change them only to correct a design by hand.'}
        </p>
        {kind === "waves" ? (
          <>
            <RowList
              id='experiment-design-waves'
              title='Waves'
              empty={
                observed
                  ? "No waves yet — add the units that changed and the date they changed."
                  : "No waves yet — price the design to get them."
              }
              addLabel='Add a wave'
              keys={waves.fields.map((f) => f.id)}
              error={p.messageFor("waves")}
              onAdd={() => waves.append(newWave())}
              onRemove={waves.remove}
              renderRow={(i) => (
                <Grid>
                  <TextField
                    {...p}
                    name={`waves.${i}.switchOn`}
                    label='Switches on'
                    type={switchType}
                  />
                  <UnitsField
                    {...p}
                    name={`waves.${i}.units`}
                    label='Covers'
                    hint='Unit (or group) names, separated by commas or new lines.'
                  />
                </Grid>
              )}
            />
            <RowList
              id='experiment-design-strata'
              title='Blocks by baseline volume'
              empty='Not blocked.'
              addLabel='Add a block'
              keys={strata.fields.map((f) => f.id)}
              error={p.messageFor("strata")}
              onAdd={() => strata.append(newStratum())}
              onRemove={strata.remove}
              renderRow={(i) => (
                <UnitsField
                  {...p}
                  name={`strata.${i}.units`}
                  label={`Block ${i + 1}`}
                  hint='Names, separated by commas or new lines.'
                />
              )}
            />
          </>
        ) : (
          <RowList
            id='experiment-design-periods'
            title='Switchback periods'
            empty='No schedule yet — price the design to get one.'
            addLabel='Add a period'
            keys={periods.fields.map((f) => f.id)}
            error={p.messageFor("periods")}
            onAdd={() => periods.append(newPeriod())}
            onRemove={periods.remove}
            renderRow={(i) => (
              <Grid>
                <TextField {...p} name={`periods.${i}.from`} label='Starts' type={switchType} />
                <SelectField
                  {...p}
                  name={`periods.${i}.state`}
                  label='Lever'
                  choices={[
                    { value: "on", label: "On" },
                    { value: "off", label: "Off" }
                  ]}
                />
              </Grid>
            )}
          />
        )}
        <Grid>
          <TextField {...p} name='seed' label='Assignment seed' type='number' min={0} step={1} />
          <TextField
            {...p}
            name='mdeAtProposal'
            label='Promised minimum detectable effect'
            type='number'
            step='any'
            hint='Per unit-day, or per group-day when groups are randomised.'
          />
          <TextField
            {...p}
            name='priorValue'
            label='World Model prior slope'
            type='number'
            step='any'
          />
          <SelectField
            {...p}
            name='priorSource'
            label='Where the slope came from'
            choices={[
              { value: "declared", label: "Declared in the World Model" },
              { value: "fitted", label: "Fitted from history" }
            ]}
          />
        </Grid>
      </div>
    </details>
  );
}
