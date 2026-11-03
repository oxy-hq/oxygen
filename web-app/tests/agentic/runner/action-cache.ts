import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

// Bump when the toolkit shape or replay semantics change so old
// entries invalidate cleanly. Cached actions recorded against an older
// version are ignored on read and filtered out at load time.
// 4: recordings no longer rank an element's ambiguous test id above the
// model's scoped selector (selectors.ts `materializeStrategies`); entries
// from 3 carry that rank 0 and fail every replay on a page that renders it twice.
export const CACHE_VERSION = 4;

export type SelectorKind = "testid" | "role_name" | "text" | "css";

export interface SelectorStrategy {
  kind: SelectorKind;
  selector: string; // literal Playwright selector string
  rank: number; // 0 = try first
}

export interface RecordedAction {
  tool: string;
  args: Record<string, unknown>;
  /** Ranked alternative selectors. Absent for non-selector tools. */
  selector_strategies?: SelectorStrategy[];
  /** Free-text "what the LLM was doing" — used by Tier 2 healing redrive. */
  intent?: string;
}

interface CacheEntry {
  version: number;
  actions: RecordedAction[];
  /** Consecutive replays without LLM redrive. Reset to 0 on cold redrive. */
  hit_streak: number;
  last_recorded_at: string;
  last_replayed_at?: string;
}

export interface CacheFile {
  entries: Record<string, CacheEntry>;
}

type CacheScope = "flow" | "shared";

interface CacheGetResult {
  actions: RecordedAction[];
  hit_streak: number;
}

export interface ActionCache {
  get(key: string): CacheGetResult | undefined;
  set(key: string, actions: RecordedAction[]): void;
  /** Update strategies for one action in-place (Tier 1 silent re-rank). */
  updateActionStrategies(key: string, actionIndex: number, strategies: SelectorStrategy[]): void;
  /** Bump the hit_streak counter on a successful replay. */
  recordReplay(key: string): void;
  invalidate(key: string): void;
  cacheKey(
    flowFile: string,
    caseName: string,
    stepIndex: number,
    stepText: string,
    scope?: CacheScope
  ): string;
}

export function createActionCache(path: string): ActionCache {
  let loaded: CacheFile | null = null;

  const ensureLoaded = (): CacheFile => {
    if (loaded) return loaded;
    if (existsSync(path)) {
      try {
        const raw = readFileSync(path, "utf-8");
        const parsed = JSON.parse(raw) as Partial<CacheFile>;
        // Drop entries from older cache schemas at load time so a later
        // set() / persist() doesn't re-serialize them. get() also skips
        // stale-version entries, but without this filter they leak back
        // out on every rewrite.
        const filtered: Record<string, CacheEntry> = {};
        for (const [k, v] of Object.entries(parsed.entries ?? {})) {
          if (v && v.version === CACHE_VERSION) filtered[k] = v;
        }
        loaded = { entries: filtered };
      } catch {
        loaded = { entries: {} };
      }
    } else {
      loaded = { entries: {} };
    }
    return loaded;
  };

  const persist = (): void => {
    const data = ensureLoaded();
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, JSON.stringify(data, null, 2), "utf-8");
  };

  return {
    get(key) {
      const entry = ensureLoaded().entries[key];
      if (!entry || entry.version !== CACHE_VERSION) return undefined;
      return { actions: entry.actions, hit_streak: entry.hit_streak ?? 0 };
    },
    set(key, actions) {
      const data = ensureLoaded();
      data.entries[key] = {
        version: CACHE_VERSION,
        actions,
        hit_streak: 0,
        last_recorded_at: new Date().toISOString()
      };
      persist();
    },
    updateActionStrategies(key, actionIndex, strategies) {
      const data = ensureLoaded();
      const entry = data.entries[key];
      if (!entry || entry.version !== CACHE_VERSION) return;
      const action = entry.actions[actionIndex];
      if (!action) return;
      action.selector_strategies = strategies;
      persist();
    },
    recordReplay(key) {
      const data = ensureLoaded();
      const entry = data.entries[key];
      if (!entry || entry.version !== CACHE_VERSION) return;
      entry.hit_streak = (entry.hit_streak ?? 0) + 1;
      entry.last_replayed_at = new Date().toISOString();
      persist();
    },
    invalidate(key) {
      const data = ensureLoaded();
      if (key in data.entries) {
        delete data.entries[key];
        persist();
      }
    },
    cacheKey(flowFile, caseName, stepIndex, stepText, scope = "flow") {
      const h = createHash("sha256");
      // `shared` scope hashes only the step prompt — two flows that
      // describe the same canonical step (e.g., a copy-pasted prelude)
      // resolve to the same entry. `flow` scope (default) keeps the
      // full path so flows stay independent.
      if (scope === "shared") {
        h.update(`shared|${stepText}`);
      } else {
        h.update(`${flowFile}|${caseName}|${stepIndex}|${stepText}`);
      }
      return h.digest("hex");
    }
  };
}
