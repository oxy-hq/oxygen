import { apiClient } from "./axios";

export type FlagUnit = "org" | "user";

export type Fallthrough =
  | { type: "variant"; variant: string }
  /** Basis points per variant; the weights sum to 10000. */
  | { type: "split"; weights: Record<string, number> };

export interface Targeting {
  /** user uuid -> variant */
  users: Record<string, string>;
  /** org uuid -> variant */
  orgs: Record<string, string>;
  /** What everyone not overridden gets. */
  fallthrough: Fallthrough;
  /** Hash salt; absent = the flag key. Read-only here. */
  salt?: string;
}

export interface FeatureFlag {
  key: string;
  description: string;
  default: boolean;
  /** The kill switch: off means every org gets `variants[0]`. */
  enabled: boolean;
  updated_at: string | null;
  /** `variants[0]` is the off/control variant; `["off", "on"]` for on/off flags. */
  variants: string[];
  /** What a split buckets on. */
  unit: FlagUnit;
  /** False: global only; the server refuses targeting (422). */
  per_org: boolean;
  /** The web app may read it. */
  client: boolean;
  /** Null: a plain global flag. */
  targeting: Targeting | null;
  /** Stored rules this build cannot serve (it serves `variants[0]`). */
  targeting_error: string | null;
}

/** Omit a field to leave it; `targeting: null` clears the rules. */
export interface FeatureFlagPatch {
  enabled?: boolean;
  targeting?: Targeting | null;
}

export type EvaluationReason =
  | "STATIC"
  | "DISABLED"
  | "TARGETING_MATCH"
  | "SPLIT"
  | "DEFAULT"
  | "ERROR";

export interface FlagEvaluation {
  key: string;
  variant: string;
  reason: EvaluationReason;
  bucket?: number;
}

export interface EvaluateQuery {
  orgId?: string;
  userId?: string;
}

export interface OrgFeatureFlags {
  flags: Record<string, { variant: string; enabled: boolean }>;
}

export class FeatureFlagsService {
  static async list(): Promise<FeatureFlag[]> {
    const response = await apiClient.get<FeatureFlag[]>("/admin/feature-flags");
    return response.data;
  }

  static async update(key: string, patch: FeatureFlagPatch): Promise<FeatureFlag> {
    const response = await apiClient.patch<FeatureFlag>(
      `/admin/feature-flags/${encodeURIComponent(key)}`,
      patch
    );
    return response.data;
  }

  /** A preview of what each flag serves that org / person. Records nothing. */
  static async evaluate({ orgId, userId }: EvaluateQuery): Promise<FlagEvaluation[]> {
    const response = await apiClient.get<FlagEvaluation[]>("/admin/feature-flags/evaluate", {
      params: { org_id: orgId || undefined, user_id: userId || undefined }
    });
    return response.data;
  }

  /** The `client` flags, evaluated server-side for the caller. Any org member. */
  static async forOrg(orgId: string): Promise<OrgFeatureFlags> {
    const response = await apiClient.get<OrgFeatureFlags>(
      `/orgs/${encodeURIComponent(orgId)}/feature-flags`
    );
    return response.data;
  }
}
