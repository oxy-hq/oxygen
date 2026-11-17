/**
 * What refuses a move to production, by the way production moves — the list the
 * "Publishing & CI" panel prints, kept as data so the claim it makes is tested rather
 * than typed into JSX.
 *
 * Production moves three ways an operator reaches from here: Promote to production
 * (also `oxyc promote`), a promoting publish (`oxyc publish --promote`, which CI may
 * send), and a rollback (also `oxyc rollback`). Each runs the blockers in
 * `crates/api-custom-apps/src/custom_apps_promote/blockers.rs`; a promoting publish
 * runs them before anything of it lands (`custom_apps_publish/production_gate.rs`).
 * Two rows differ by path, and both on purpose — a rollback is the emergency path:
 * it only warns about an unset required secret, and the second-person rule applies
 * to it only when its target is a build production never served (the console's
 * rollback takes any build staging served, which would otherwise be a promote by
 * another name).
 */

export type ProductionPath = "promote" | "promoting_publish" | "rollback";

export interface ProductionGate {
  /** The blocker's code, as the server's plan and refusals name it. */
  code: string;
  /** Completes "refuses a build that …". */
  refuses: string;
  /** The paths it refuses on. */
  refusesOn: ProductionPath[];
  /** Paths where it is only a warning, said beside the row. */
  warnsOn?: ProductionPath[];
}

const EVERY_PATH: ProductionPath[] = ["promote", "promoting_publish", "rollback"];
const PROMOTES: ProductionPath[] = ["promote", "promoting_publish"];

/** Every gate, in the order the server lists its blockers; the second-person one only while the app's rule is on. */
export function productionGates(requireSecondPerson: boolean): ProductionGate[] {
  const gates: ProductionGate[] = [
    {
      code: "staging_never_served",
      refuses: "staging never served (a promoting publish serves it on staging first)",
      refusesOn: EVERY_PATH
    },
    { code: "validation_not_passed", refuses: "has not passed validation", refusesOn: EVERY_PATH },
    {
      code: "required_secrets_unset",
      refuses: "declares a required secret production has no value for",
      refusesOn: PROMOTES,
      warnsOn: ["rollback"]
    },
    {
      code: "schedule_removal_unconfirmed",
      refuses: "would retire a schedule nobody confirmed (--remove-schedule <function>)",
      refusesOn: EVERY_PATH
    }
  ];
  if (requireSecondPerson) {
    gates.push({
      code: "same_person_promote",
      refuses: "the person promoting it published — the same user, CI workflow or token",
      refusesOn: PROMOTES
    });
  }
  return gates;
}

/** The codes that refuse a move along `path`. */
export function gatesFor(path: ProductionPath, requireSecondPerson: boolean): string[] {
  return productionGates(requireSecondPerson)
    .filter((gate) => gate.refusesOn.includes(path))
    .map((gate) => gate.code);
}

/** One list item: the gate, and where it does not refuse. */
export function gateLine(gate: ProductionGate): string {
  if (gate.warnsOn?.includes("rollback")) return `${gate.refuses} (a rollback only warns)`;
  if (!gate.refusesOn.includes("rollback")) {
    return `${gate.refuses} (a rollback only to a build production never served)`;
  }
  return gate.refuses;
}

/** What the second-person rule does, said for the state it is in. */
export function secondPersonConsequence(on: boolean): string {
  return on
    ? "On: production refuses a promote by whoever published the build. A promoting publish (--promote) is refused every time — CI publishes to staging and someone else promotes. A rollback to a build production served before stays open to anyone, for emergencies; its publisher's rollback to one production never served is refused like a promote."
    : "Off: whoever published a build may promote it, and oxyc publish --promote moves production straight from CI. Turn on to have a second person promote every build.";
}
