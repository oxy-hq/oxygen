/**
 * Which manifest capability each `ctx.<area>.<op>` needs — the CLI's copy of
 * the fail-closed gates in
 * `crates/app/src/server/api/custom_apps_functions/host.rs`.
 *
 * ONE SOURCE OF TRUTH, AND IT IS NOT THIS FILE. `FunctionCapabilities` in
 * `host.rs` is the struct every gate reads: each `ctx.*` op refuses with
 * `<Area>CapabilityMissing` when its field is false, `check_storage_capability`
 * is the op-by-op split for `ctx.storage`, and `check_write_destination` plus
 * `host/destinations.rs` decide a `ctx.warehouse` / `ctx.tx` write. This file
 * names those fields so `oxyc validate` and `oxyc publish` can answer, before
 * the upload, what the host would answer at the first call.
 *
 * HELD TO THE STRUCT BY A RUST TEST. `cli_capabilities_drift.rs`, beside
 * `host.rs`, `include_str!`s this file and fails when the `hostField` values
 * below stop naming exactly the gate fields `FunctionCapabilities` carries, or
 * when the `storage.*` ops here stop matching `check_storage_capability`'s
 * arms. A gate added to the host without an entry here fails that test; an
 * entry here for a gate the host no longer has fails it too.
 *
 * THE RUST TEST READS TEXT, NOT TYPESCRIPT: after stripping comments it takes
 * every line starting `hostField:` and the `ops: […]` array after it (wrapped
 * across lines or not). Keep those two keys spelled as they are, and keep a
 * `hostField` out of any comment that is not a full-line one.
 */

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** `spec.<a>.<b> === true`. */
function flag(spec: Record<string, unknown>, a: string, b: string): boolean {
  const block = spec[a];
  return isObject(block) && block[b] === true;
}

export interface GatedCapability {
  /** The `FunctionCapabilities` field in host.rs that gates these ops. */
  readonly hostField: string;
  /** The capability as the docs and the host's refusals name it: `storage.write`. */
  readonly manifest: string;
  /** What to add to the function's oxy-app.json entry — the fix. */
  readonly declare: string;
  /**
   * The `ctx` members that need this gate, as paths under `ctx`:
   * `storage.put`, `oltp.*` (every op of that area), `tx` (`ctx.tx(...)` itself).
   */
  readonly ops: readonly string[];
  /**
   * `manifest`: the gate is a flag on the function's entry, and `declares`
   * answers from the manifest alone.
   *
   * `destination`: the gate depends on WHICH database the call names and what
   * kind of store it is — a customer warehouse needs the reason, Airhouse does
   * not, the org's OLTP is refused outright — and only the server knows the
   * kind. `function-engines.ts` answers at publish; offline, `function-lint.ts`
   * checks the `destinations` allowlist and says the rest was skipped.
   */
  readonly gate: "manifest" | "destination";
  /** True when the function's manifest entry declares the capability. */
  declares(spec: Record<string, unknown>, database?: string): boolean;
}

/**
 * Every gate in `FunctionCapabilities`, in the struct's order.
 *
 * `storage_retention` and `fetch_max_bytes` are also fields of that struct and
 * are deliberately absent here: a retention policy and a byte ceiling shape a
 * call that is allowed, they never refuse one. The Rust test carries the same
 * two names as its exclusion list, so a third non-gate field has to be added
 * in both places on purpose.
 */
export const GATED_CAPABILITIES: readonly GatedCapability[] = [
  {
    hostField: "secrets_write",
    manifest: "secrets.write",
    declare: '"secrets": { "write": true }',
    ops: ["secrets.set"],
    gate: "manifest",
    declares: (spec) => flag(spec, "secrets", "write")
  },
  {
    hostField: "email_send",
    manifest: "email.send",
    declare: '"email": { "send": true }',
    ops: ["email.send"],
    gate: "manifest",
    declares: (spec) => flag(spec, "email", "send")
  },
  {
    hostField: "org_read",
    manifest: "org.read",
    declare: '"org": { "read": true }',
    ops: ["org.people", "org.places", "org.assignments"],
    gate: "manifest",
    declares: (spec) => flag(spec, "org", "read")
  },
  {
    hostField: "storage_read",
    manifest: "storage.read",
    declare: '"storage": { "read": true }',
    ops: ["storage.getDownloadUrl", "storage.get", "storage.head", "storage.list", "storage.copy"],
    gate: "manifest",
    declares: (spec) => flag(spec, "storage", "read")
  },
  {
    hostField: "storage_write",
    manifest: "storage.write",
    declare: '"storage": { "write": true }',
    ops: ["storage.getUploadUrl", "storage.put", "storage.delete", "storage.copy"],
    gate: "manifest",
    declares: (spec) => flag(spec, "storage", "write")
  },
  {
    hostField: "oltp",
    manifest: "oltp.enabled",
    declare: '"oltp": { "enabled": true }',
    ops: ["oltp.*"],
    gate: "manifest",
    declares: (spec) => flag(spec, "oltp", "enabled")
  },
  {
    hostField: "airhouse",
    manifest: "airhouse.enabled",
    declare: '"airhouse": { "enabled": true }',
    ops: ["airhouse.*"],
    gate: "manifest",
    declares: (spec) => flag(spec, "airhouse", "enabled")
  },
  {
    hostField: "customer_warehouse_writes",
    manifest: "customerWarehouseWrites",
    declare: '"customerWarehouseWrites": { "<database>": "<why this write stays there>" }',
    ops: ["warehouse.insert", "warehouse.exec", "warehouse.upsert", "tx"],
    gate: "destination",
    declares: (spec, database) => {
      const writes = spec.customerWarehouseWrites;
      if (database === undefined || !isObject(writes)) return false;
      const reason = writes[database];
      return typeof reason === "string" && reason.trim() !== "";
    }
  }
];

/**
 * The `ctx.warehouse` / `ctx.tx` writes, which also need the database in the
 * function's `destinations` — `write_destinations` on the host, checked by
 * `check_write_destination` before any capability: "an empty allowlist denies
 * every database". Not a `FunctionCapabilities` field, so not in the map above.
 */
export const WRITE_OPS: readonly string[] = GATED_CAPABILITIES.filter(
  (c) => c.gate === "destination"
).flatMap((c) => c.ops);

/** The gates a `ctx` member path needs — usually one; `storage.copy` needs two. */
export function capabilitiesFor(member: string): GatedCapability[] {
  const area = member.split(".")[0] ?? member;
  return GATED_CAPABILITIES.filter((c) => c.ops.includes(member) || c.ops.includes(`${area}.*`));
}
