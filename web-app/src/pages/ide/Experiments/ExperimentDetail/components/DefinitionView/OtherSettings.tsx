import HowCalculated from "../ResultPane/components/HowCalculated";
import { humanize } from "./describe";
import { Block, Fact, Facts } from "./shared";

const asText = (v: unknown) =>
  typeof v === "object" && v !== null ? JSON.stringify(v) : String(v as string | number | boolean);

/** Keys this build does not know, so nothing the server sends is hidden. */
export default function OtherSettings({ extra }: { extra: Record<string, unknown> }) {
  const keys = Object.keys(extra);
  if (keys.length === 0) return null;
  return (
    <Block id='other' title='Other settings'>
      <HowCalculated
        title={`Show ${keys.length} other ${keys.length === 1 ? "setting" : "settings"}`}
      >
        <Facts>
          {keys.map((k) => (
            <Fact key={k} label={humanize(k)}>
              <span className='whitespace-pre-wrap'>
                {extra[k] === null ? "Not set" : asText(extra[k])}
              </span>
            </Fact>
          ))}
        </Facts>
      </HowCalculated>
    </Block>
  );
}
