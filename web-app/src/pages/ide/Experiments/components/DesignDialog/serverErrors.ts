/**
 * The server answers a refused design in sentences that name the config's keys
 * (`outcome.alpha is 1.5, which is not in (0, 1)`). Each sentence is placed
 * beside the form field it is about, with the key replaced by that field's
 * label; a sentence about no field goes at the top of the form.
 */

/** Config key path -> [form field, the label that field shows]. Longest paths first. */
const PATHS: [string, string, string][] = [
  ["outcome.prior_coefficient.value", "priorValue", "World Model prior slope"],
  ["outcome.prior_coefficient", "priorValue", "World Model prior slope"],
  ["design.switchback.period_days", "periodDays", "Period length"],
  ["design.switchback.periods", "periods", "Switchback periods"],
  ["design.switchback", "kind", "Switchback"],
  ["lever.read_back.grace_days", "graceDays", "Days allowed to confirm a switch"],
  ["lever.read_back.when_on", "whenOn", "Confirmed on when"],
  ["lever.read_back.when_off", "whenOff", "Confirmed off when"],
  ["lever.on_close", "onClose", "At close"],
  ["lever.per", "per", "How runs are made"],
  ["lever.on", "onVia", "Turned on by"],
  ["lever.off", "offVia", "Turned off by"],
  ["design.anticipation_days", "anticipationDays", "Anticipation buffer"],
  ["design.washout_days", "washoutDays", "Washout"],
  ["design.pre_days", "preDays", "Days measured before each switch"],
  ["design.post_days", "postDays", "Days measured after each switch"],
  ["design.coverage_floor", "coverageFloor", "Days with data in each window"],
  ["design.compliance_floor", "complianceFloor", "Switches that must land"],
  ["design.time_dimension", "timeDimension", "Calendar date dimension"],
  ["design.timezone", "timezone", "Time zone"],
  ["design.comparison", "comparison", "Compare the changed units with"],
  ["design.one_date_test", "oneDateTest", "Test for one change date"],
  ["design.assignment", "assignment", "How the treated units are chosen"],
  ["design.exclude", "exclude", "Keep out of the comparison"],
  ["design.strata", "strata", "Blocks"],
  ["design.waves", "waves", "Waves"],
  ["design.seed", "seed", "Assignment seed"],
  ["outcome.mde_at_proposal", "mdeAtProposal", "Promised minimum detectable effect"],
  ["outcome.direction", "direction", "Expected direction"],
  ["outcome.family", "family", "Outcomes registered together"],
  ["outcome.target", "target", "Outcome measure"],
  ["outcome.driver", "driver", "Driver"],
  ["outcome.alpha", "alpha", "Significance level"],
  ["outcome.power", "power", "Power"],
  ["substitution_check.sibling", "sibling", "Watch for cannibalisation of"],
  ["substitution_check", "sibling", "Watch for cannibalisation of"],
  ["assign_by", "assignBy", "Randomise groups by"],
  ["for_each", "forEach", "Randomises each"],
  ["hypothesis", "hypothesis", "Hypothesis"],
  ["experiment", "experiment", "Experiment name"],
  ["owner", "owner", "Owned by"]
];

/** Words of the file format that mean nothing on a form. */
const WORDS: [RegExp, string][] = [
  [/\bswitch_on\b/g, "switch date"],
  [/\bsynthetic_control\b/g, "a synthetic control"],
  [/\bmust be observed\b/g, "must be a change that already happened"],
  [/\bunder assign_by\b/g, "when groups are randomised"]
];

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
/**
 * A key path as a whole token: `config.` prefixes and backticks around it are
 * allowed. A one-word key (`owner`) is also an English word, so it counts only
 * when the sentence marks it as a key — backticked, or after `config.`.
 */
const tokenFor = (path: string) =>
  path.includes(".")
    ? new RegExp(`(?<![\\w.])\`?(?:config\\.)?${escapeRe(path)}(?![\\w])\`?`, "g")
    : new RegExp(`\`${escapeRe(path)}\`|(?<![\\w.])config\\.${escapeRe(path)}(?![\\w.])`, "g");

const GUARDRAIL = /`?(?:config\.)?guardrails\[(\d+)\][\w.[\]]*`?/g;

/** The field a sentence is about: the key it names first (ties go to the longer key). */
function fieldOf(sentence: string): string | null {
  let best: { at: number; field: string } | null = null;
  const consider = (at: number, field: string) => {
    if (best === null || at < best.at) best = { at, field };
  };
  for (const [path, field] of PATHS) {
    const m = tokenFor(path).exec(sentence);
    if (m) consider(m.index, field);
  }
  const g = new RegExp(GUARDRAIL.source).exec(sentence);
  if (g) consider(g.index, `guardrails.${g[1]}.significance`);
  const found = best as { at: number; field: string } | null;
  if (found) return found.field;
  if (/\bexperiment named .* already exists/.test(sentence)) return "experiment";
  if (/\bwaves? \d+\b/.test(sentence)) return "waves";
  if (/\bstrat(um|a)\b/.test(sentence)) return "strata";
  return null;
}

/** The sentence with every key it names replaced by the label the form shows. */
export function plainSentence(sentence: string): string {
  let out = sentence.replace(GUARDRAIL, (_, i: string) => `guardrail ${Number(i) + 1}`);
  for (const [path, , label] of PATHS) out = out.replace(tokenFor(path), `"${label}"`);
  for (const [re, word] of WORDS) out = out.replace(re, word);
  return out.charAt(0).toUpperCase() + out.slice(1);
}

export interface PlacedErrors {
  /** Form field -> sentences about it, joined. */
  fields: Record<string, string>;
  /** Sentences about no field. */
  general: string[];
}

/** One server message, split into its sentences and placed. */
export function placeServerError(message: string): PlacedErrors {
  const body = message.replace(/^the design is not valid:\s*/i, "");
  const sentences = body === message ? [message] : body.split(/;\s+/);
  const placed: PlacedErrors = { fields: {}, general: [] };
  for (const sentence of sentences) {
    const field = fieldOf(sentence);
    const text = plainSentence(sentence.trim());
    if (field === null) placed.general.push(text);
    else placed.fields[field] = placed.fields[field] ? `${placed.fields[field]} ${text}` : text;
  }
  return placed;
}
