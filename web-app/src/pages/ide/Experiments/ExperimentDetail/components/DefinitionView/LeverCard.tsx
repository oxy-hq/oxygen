import HowCalculated from "../ResultPane/components/HowCalculated";
import { closeText, perText, plural } from "./describe";
import { orDash, type SafeDefinition } from "./safe";
import { Block, Chip, LabelledChip } from "./shared";

export default function LeverCard({ lever }: { lever: SafeDefinition["lever"] }) {
  const on = lever?.on?.via;
  const off = lever?.off?.via;
  const rb = lever?.read_back;
  const behaviour = [
    lever?.per ? `${perText(lever.per)}.` : "",
    lever?.on_close ? `${closeText(lever.on_close)}.` : ""
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <Block id='lever' title='Lever' hint={behaviour || undefined}>
      <div className='flex flex-wrap items-center gap-1.5'>
        {on !== undefined && on === off ? (
          <LabelledChip label='Turned on and off by'>{on}</LabelledChip>
        ) : (
          <>
            <LabelledChip label='Turned on by'>{orDash(on)}</LabelledChip>
            <LabelledChip label='Turned off by'>{orDash(off)}</LabelledChip>
          </>
        )}
      </div>
      <HowCalculated title='Read-back conditions'>
        <p>
          A switch counts as confirmed when the data shows it, within{" "}
          {orDash(rb?.grace_days, (n) => plural(n, "day"))}:
        </p>
        <ul className='space-y-1'>
          <li>
            On: <Chip>{orDash(rb?.when_on)}</Chip>
          </li>
          <li>
            Off: <Chip>{orDash(rb?.when_off)}</Chip>
          </li>
        </ul>
      </HowCalculated>
    </Block>
  );
}
