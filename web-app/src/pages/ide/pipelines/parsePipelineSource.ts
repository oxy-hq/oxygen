import YAML from "yaml";

export interface QuickBooksPipelineSource {
  kind: "quickbooks";
  clientId: string;
  clientSecretVar: string;
  refreshTokenVar: string;
}

/**
 * A hand-written YAML scalar as text. A mapping, list or boolean where an id or a
 * variable name belongs reads as missing, rather than as `[object Object]`.
 */
const scalarText = (value: unknown): string =>
  typeof value === "string" || typeof value === "number" ? String(value) : "";

/**
 * Parse a `.airway.yml` and return its QuickBooks source config, or `null`
 * if the pipeline isn't a QuickBooks source (or the required fields are
 * missing / the YAML is unparsable). Used to drive the Reconnect button.
 */
export function parseQuickBooksSource(yamlText: string): QuickBooksPipelineSource | null {
  let doc: unknown;
  try {
    doc = YAML.parse(yamlText);
  } catch {
    return null;
  }
  const source = (doc as { source?: { kind?: unknown; config?: Record<string, unknown> } })?.source;
  if (source?.kind !== "quickbooks") return null;
  const config = source.config ?? {};
  const clientId = scalarText(config.client_id);
  const clientSecretVar = scalarText(config.client_secret_var);
  const refreshTokenVar = scalarText(config.refresh_token_var);
  if (!clientId || !clientSecretVar || !refreshTokenVar) return null;
  return { kind: "quickbooks", clientId, clientSecretVar, refreshTokenVar };
}
