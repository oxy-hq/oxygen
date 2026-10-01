import type { AirwayDeploymentValues } from "@/services/api/airwayConfig";

type DeploymentFieldKey = keyof AirwayDeploymentValues;

/**
 * Sentinel draft value for "no explicit value — take airway's built-in
 * default". The empty string, so an emptied input and an unset setting are the
 * same state and neither can be mistaken for `0`.
 */
export const UNSET = "";

export interface DeploymentField {
  key: DeploymentFieldKey;
  label: string;
  kind: "integer" | "decimal" | "text" | "boolean";
  /** Rendered beside the label. The stated unit — never converted anywhere. */
  unit?: string;
  help: string;
  group: "transport" | "retry" | "extraction" | "tls";
}

/**
 * The eight `GlobalConfig` settings, as the eleven fields that carry them.
 *
 * **Exactly these.** `max_rewind`, `allow_unversioned_writes` and
 * `partition_repull_budget` are deliberately absent: they have no reader in
 * airway, so a control for one would be accepted, saved, and inert — the
 * failure this whole surface exists to avoid. `cursor_lag_floor` was on that
 * list until airway 0.1.24 gave it one, which is the rule working rather than
 * bending: a control appears when a consumer does.
 *
 * `tls_server_name` and `tls_enabled` are absent for the same reason one level
 * down: airway itself withholds those keys because its only consumer cannot
 * honour them.
 */
export const DEPLOYMENT_FIELDS: DeploymentField[] = [
  {
    key: "timeout_secs",
    label: "Request timeout",
    kind: "integer",
    unit: "seconds",
    group: "transport",
    help: "Per-request deadline for airway's HTTP sources. Not a total run deadline — `airway load` applies it as an inactivity gap so a working slow download is not aborted."
  },
  {
    key: "user_agent",
    label: "User agent",
    kind: "text",
    group: "transport",
    help: "Outbound identity for clients airway builds through its shared builder. Some vendors reject an empty one, so clear the field to keep airway's identity rather than saving a blank."
  },
  {
    key: "max_retries",
    label: "Max retries",
    kind: "integer",
    unit: "attempts",
    group: "retry",
    help: "One count for both of airway's retry layers — it deliberately does not split them. 0 is a real choice here: it means do not retry."
  },
  {
    key: "retry_initial_delay_ms",
    label: "Initial retry delay",
    kind: "integer",
    unit: "milliseconds",
    group: "retry",
    help: "First backoff delay. The sequence is delay × factor, so airway refuses 0 — it would stay 0 forever, a hot loop rather than a backoff."
  },
  {
    key: "retry_max_delay_secs",
    label: "Max retry delay",
    kind: "integer",
    unit: "seconds",
    group: "retry",
    help: "Ceiling on the backoff. airway refuses 0, which would collapse every delay to nothing."
  },
  {
    key: "retry_backoff_factor",
    label: "Backoff factor",
    kind: "decimal",
    group: "retry",
    help: "Multiplier between attempts. Must be at least 1 — anything smaller shrinks the delay instead of growing it."
  },
  {
    key: "cursor_lag_floor_secs",
    label: "Cursor lag floor",
    kind: "integer",
    unit: "seconds",
    group: "extraction",
    help: "Raises every resource's declared cursor lag to at least this, for sources whose index lags further back than their contract claims. A floor, never a ceiling — it can only widen a window, never narrow one a vendor needs. Leave empty for no floor; airway refuses 0, which would raise nothing and so says nothing."
  },
  {
    key: "tls_ca_cert",
    label: "CA certificate",
    kind: "text",
    group: "tls",
    help: "Path, on the airway worker's filesystem, to a PEM trust anchor."
  },
  {
    key: "tls_client_cert",
    label: "Client certificate",
    kind: "text",
    group: "tls",
    help: "Path to a PEM client certificate. Must be set together with the client key — half an mTLS identity yields no identity at all."
  },
  {
    key: "tls_client_key_file",
    label: "Client key file",
    kind: "text",
    group: "tls",
    help: "Path to the PEM private key for the client certificate above."
  },
  {
    key: "tls_danger_accept_invalid_certs",
    label: "Accept invalid certificates",
    kind: "boolean",
    group: "tls",
    help: "Skips server certificate verification. Insecure — development only, and process-wide: it applies to every airway source in every workspace this deployment serves, not to one connection. On its own this does not configure a trust store."
  }
];

export const GROUP_LABELS: Record<DeploymentField["group"], string> = {
  transport: "Transport",
  retry: "Retry",
  extraction: "Extraction",
  tls: "TLS"
};

export type DeploymentDraft = Record<DeploymentFieldKey, string>;

/** Render a stored value as draft text. `null` becomes [`UNSET`], never `"0"`. */
export function draftFromValues(values: AirwayDeploymentValues): DeploymentDraft {
  const draft = {} as DeploymentDraft;
  for (const field of DEPLOYMENT_FIELDS) {
    const value = values[field.key];
    draft[field.key] = value === null || value === undefined ? UNSET : String(value);
  }
  return draft;
}

export interface ParsedDraft {
  values: AirwayDeploymentValues;
  /** Keys whose text is not a value of the field's kind. Blocks Save. */
  invalid: DeploymentFieldKey[];
}

/**
 * Parse a draft back into the wire shape.
 *
 * [`UNSET`] becomes `null` — "airway's default" — for every kind. That is the
 * one rule this whole module exists to hold: an emptied number input must not
 * become `0`, and an emptied text input must not become `""` (airway rejects
 * an empty `user_agent` outright rather than reading it as unset).
 *
 * Value *rules* are not checked here on purpose. airway owns them, the API
 * enforces them on write, and a second copy in the browser is a copy that goes
 * stale; only "is this text a number at all" is decided locally, because that
 * is a property of the input rather than of the setting.
 */
export function valuesFromDraft(draft: DeploymentDraft): ParsedDraft {
  // Accumulated loosely and cast once at the return, rather than five
  // per-assignment escapes: this is one loop over a heterogeneous record, and
  // TypeScript cannot connect `field.kind` to `field.key`'s value type. The
  // cast is sound because the loop covers exactly `DEPLOYMENT_FIELDS`, which
  // is exactly `AirwayDeploymentValues`' keys — pinned by the round-trip test
  // and by the "no knob airway does not read" test.
  const values: Record<string, string | number | boolean | null> = {};
  const invalid: DeploymentFieldKey[] = [];

  for (const field of DEPLOYMENT_FIELDS) {
    const raw = draft[field.key].trim();
    if (raw === UNSET) {
      values[field.key] = null;
      continue;
    }
    if (field.kind === "boolean") {
      values[field.key] = raw === "true";
      continue;
    }
    if (field.kind === "text") {
      values[field.key] = raw;
      continue;
    }
    // `Number`, not `parseFloat`/`parseInt`: those stop at the first bad
    // character, so `"30s"` would silently save as 30.
    const parsed = Number(raw);
    const ok =
      Number.isFinite(parsed) &&
      (field.kind === "decimal" || Number.isInteger(parsed)) &&
      parsed >= 0;
    if (!ok) {
      invalid.push(field.key);
      values[field.key] = null;
      continue;
    }
    values[field.key] = parsed;
  }

  return { values: values as unknown as AirwayDeploymentValues, invalid };
}

/**
 * Whether the draft differs from what is stored — arms Save.
 *
 * Compares **parsed values**, not the text that produced them. Comparing text
 * armed Save forever after a successful save: the draft keeps the operator's
 * literal spelling, the row round-trips normalised, and `1.50` never again
 * equals the stored `1.5`. The operator is then shown an enabled Save for a
 * change that has already been made, and clicking it writes the same row.
 *
 * A field whose text is not a value of its kind parses to `null` here, so it
 * can read as clean against a stored `null`. That is harmless: `invalid` is
 * non-empty in exactly that case and disables Save on its own.
 */
export function isDirty(draft: DeploymentDraft, stored: AirwayDeploymentValues): boolean {
  const { values } = valuesFromDraft(draft);
  return DEPLOYMENT_FIELDS.some((f) => (values[f.key] ?? null) !== (stored[f.key] ?? null));
}
