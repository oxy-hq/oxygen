import { describe, expect, it } from "vitest";
import { isValidFunctionName, isValidSlug } from "./manifest-rules";
import { isV1Manifest, type ManifestIssue, validateOxyAppManifest } from "./manifest-validate";

const errors = (raw: unknown): ManifestIssue[] =>
  validateOxyAppManifest(raw).filter((i) => i.level === "error");
const warnings = (raw: unknown): ManifestIssue[] =>
  validateOxyAppManifest(raw).filter((i) => i.level === "warning");
const messages = (raw: unknown): string[] => errors(raw).map((i) => i.message);

describe("validateOxyAppManifest", () => {
  it("accepts a full manifest", () => {
    expect(
      validateOxyAppManifest({
        $schema: "https://unpkg.com/@oxy-hq/sdk/schema/oxy-app.schema.json",
        schemaVersion: 2,
        slug: "store-ops",
        name: "Store Ops",
        description: "Find your next location",
        icon: "icon.svg",
        art: "card.svg",
        status: "23 stores · live",
        orgSlug: "acme",
        build: { install: "pnpm install", command: "pnpm build", outDir: "out" },
        environments: { eu: { target: "https://eu.example.test" } },
        functions: {
          sync: {
            schedule: "0 * * * *",
            timezone: "UTC",
            timeoutSeconds: 120,
            rateLimitPerMinute: 10,
            fetch: { maxResponseBytes: 50_000_000 },
            cache: { ttlSeconds: 60 },
            destinations: ["airhouse"],
            airhouse: { enabled: true },
            storage: { read: true, write: true },
            retries: { maxAttempts: 3, minTimeoutMs: 100, maxTimeoutMs: 1000 },
            notify: { kinds: { late: { critical: true } } },
            inputExample: { since: "2026-01-01" }
          },
          ingest: {
            route: false,
            webhook: { secretVar: "KEYS", signatureHeader: "x-sig", encoding: "base64" }
          }
        },
        migrations: { dir: "migrations" },
        storage: { retention: [{ prefix: "tmp/", expireAfter: "1d" }, { prefix: "keep/" }] },
        performance: { serviceWorker: false },
        analytics: false,
        env: { STRIPE_API_KEY: { required: true, description: "Restricted key" } },
        notifications: { kinds: { late: { level: "critical" } } }
      })
    ).toEqual([]);
  });

  it("names a non-object", () => {
    expect(validateOxyAppManifest([])).toEqual([
      { level: "error", path: "(root)", message: "the top level must be an object" }
    ]);
  });

  it("refuses a v1 manifest in one sentence per shape", () => {
    expect(messages({ schemaVersion: 1, slug: "x", products: {} })).toEqual([
      "schemaVersion must be 2 (got 1); v1 manifests are no longer supported — upgrade to the identity-only shape",
      "oxy-app.json is schemaVersion 2 (identity-only); the v1 fields `products` and `writers` are no longer supported"
    ]);
    expect(messages({ slug: "x" })[0]).toMatch(/schemaVersion must be 2 \(got undefined\)/);
    expect(messages({ schemaVersion: 2, slug: "x", writers: [] })).toEqual([
      "oxy-app.json is schemaVersion 2 (identity-only); the v1 fields `products` and `writers` are no longer supported"
    ]);
    // Reported as v1, not as two unknown fields.
    expect(warnings({ schemaVersion: 2, slug: "x", products: {}, writers: [] })).toEqual([]);
    expect(isV1Manifest({ schemaVersion: 1 })).toBe(true);
    expect(isV1Manifest({ schemaVersion: 2, slug: "x" })).toBe(false);
  });

  it("requires a non-empty slug and refuses a malformed one", () => {
    for (const slug of [undefined, "", "   ", 7]) {
      expect(messages({ schemaVersion: 2, slug })).toEqual([
        "`slug` is required and must be a non-empty string"
      ]);
    }
    for (const slug of ["MyApp", "-bad", "app-", "a--b", "my_app", "a".repeat(64)]) {
      const [only] = messages({ schemaVersion: 2, slug });
      expect(only, slug).toMatch(/^`slug` ".*" is invalid — use 1–63 lowercase/);
      expect(only, slug).toContain("no underscore");
      expect(only, slug).toContain("malformed");
      expect(isValidSlug(slug)).toBe(false);
    }
    expect(messages({ schemaVersion: 2, slug: "oltp-bookings" })).toEqual([]);
  });

  it("checks function names and shapes", () => {
    expect(messages({ schemaVersion: 2, slug: "x", functions: [] })).toEqual([
      "functions must be an object keyed by function name"
    ]);
    const [bad] = errors({ schemaVersion: 2, slug: "x", functions: { Bad_Name: {} } });
    expect(bad).toMatchObject({ path: "functions.Bad_Name" });
    expect(bad?.message).toMatch(/^function name "Bad_Name" must match \^\[a-z\]/);
    expect(bad?.message).toContain("malformed");
    expect(isValidFunctionName("Bad_Name")).toBe(false);
    expect(messages({ schemaVersion: 2, slug: "x", functions: { f: "nope" } })).toEqual([
      'function "f" must be an object'
    ]);
  });

  it("names the field and the fix for each constraint", () => {
    const fn = (spec: unknown) =>
      messages({ schemaVersion: 2, slug: "x", functions: { sync: spec } });
    expect(fn({ timeoutSeconds: 301 })).toEqual([
      'function "sync" `timeoutSeconds` must be an integer in [1, 300]'
    ]);
    expect(fn({ timeoutSeconds: 1.5 })).toEqual([
      'function "sync" `timeoutSeconds` must be an integer in [1, 300]'
    ]);
    expect(fn({ check: "yes" })).toEqual(['function "sync" `check` must be a boolean']);
    expect(fn({ cache: { ttlSeconds: 0 } })).toEqual([
      'function "sync" `cache.ttlSeconds` must be a positive integer'
    ]);
    expect(fn({ retries: { maxAttempts: 0 } })).toEqual([
      'function "sync" `retries.maxAttempts` must be a positive integer'
    ]);
    expect(fn({ rateLimitPerMinute: "60" })).toEqual([
      'function "sync" `rateLimitPerMinute` must be a positive integer'
    ]);
    expect(fn({ fetch: { maxResponseBytes: -1 } })).toEqual([
      'function "sync" `fetch.maxResponseBytes` must be a positive integer'
    ]);
    expect(fn({ entry: "" })).toEqual(['function "sync" `entry` must be a non-empty string']);
    expect(fn({ airwayStep: { pipeline: "p" } })).toEqual([
      'function "sync" `airwayStep.resource` is required'
    ]);
    expect(fn({ destinations: "airhouse" })).toEqual([
      'function "sync" `destinations` must be an array'
    ]);
    expect(fn({ destinations: [1] })).toEqual([
      'function "sync" `destinations.0` must be a string'
    ]);
  });

  it("refuses a half-written webhook block", () => {
    const fn = (webhook: unknown) =>
      messages({ schemaVersion: 2, slug: "x", functions: { ingest: { route: false, webhook } } });
    expect(fn("not-an-object")).toEqual(['function "ingest" `webhook` must be an object']);
    expect(fn({ signatureHeader: "x-sig" })).toEqual([
      'function "ingest" `webhook.secretVar` is required'
    ]);
    expect(fn({ secretVar: "  ", signatureHeader: "x-sig" })).toEqual([
      'function "ingest" `webhook.secretVar` must be a non-empty string'
    ]);
    expect(fn({ secretVar: "K", signatureHeader: "x-sig", encoding: "base32" })).toEqual([
      'function "ingest" `webhook.encoding` must be "hex" or "base64"'
    ]);
    expect(fn({ secretVar: "K", signatureHeader: "x-sig" })).toEqual([]);
  });

  it("requires an invocation surface", () => {
    expect(
      messages({ schemaVersion: 2, slug: "x", functions: { idle: { route: false } } })
    ).toEqual(['function "idle" must enable at least one of route/schedule/airwayStep/webhook']);
    for (const spec of [{}, { schedule: "* * * * *", route: false }, { route: true }]) {
      expect(messages({ schemaVersion: 2, slug: "x", functions: { f: spec } })).toEqual([]);
    }
  });

  describe("app events", () => {
    const base = { schemaVersion: 2, slug: "x" };

    it("accepts a function whose only trigger is on", () => {
      expect(
        messages({
          ...base,
          functions: { handle: { route: false, on: "statements/statement.uploaded" } }
        })
      ).toEqual([]);
      expect(
        messages({
          ...base,
          functions: { handle: { route: false, on: ["statements/statement.uploaded", "b/c"] } }
        })
      ).toEqual([]);
    });

    it("accepts a function that runs on a table, beside event types", () => {
      for (const on of [
        { table: "pokehouse:bookkeeping.platform_report_lines" },
        { table: "app_x.lines" },
        ["statements/statement.uploaded", { table: "pokehouse:bookkeeping.lines" }]
      ]) {
        expect(messages({ ...base, functions: { handle: { route: false, on } } })).toEqual([]);
      }
    });

    it("refuses a table written without its schema and table", () => {
      const found = errors({
        ...base,
        functions: { handle: { route: false, on: [{ table: "lines" }, { table: "a:b.c" }] } }
      });
      // The schema says no shape fits; the rule says which entry and why.
      expect(found.map((f) => f.path)).toEqual([
        "functions.handle.on",
        "functions.handle.on.0.table"
      ]);
      expect(found[1]?.message).toContain("<database>:<schema>.<table>");
      const single = errors({
        ...base,
        functions: { handle: { route: false, on: { table: "x" } } }
      });
      expect(single.map((f) => f.path)).toContain("functions.handle.on.table");
    });

    it("accepts declared events published by a function", () => {
      expect(
        messages({
          ...base,
          events: { "statement.uploaded": { description: "A statement landed", schema: "s.json" } },
          functions: { f: { events: { publish: ["statement.uploaded"] } } }
        })
      ).toEqual([]);
    });

    it("refuses events.publish naming an undeclared event", () => {
      const found = errors({
        ...base,
        functions: { f: { events: { publish: ["statement.uploaded"] } } }
      });
      expect(found).toHaveLength(1);
      expect(found[0]?.path).toBe("functions.f.events.publish.0");
      expect(found[0]?.message).toContain("statement.uploaded");
      expect(found[0]?.message).toContain("events");
    });

    it("refuses an unqualified on", () => {
      const found = errors({
        ...base,
        functions: { f: { route: false, on: ["statement.uploaded"] } }
      });
      expect(found).toHaveLength(1);
      expect(found[0]?.path).toBe("functions.f.on.0");
      expect(found[0]?.message).toContain("<app-slug>/<event-name>");
    });

    it("refuses a malformed event name in the catalogue", () => {
      expect(errors({ ...base, events: { "Bad-Name": { schema: "s.json" } } })).toHaveLength(1);
    });
  });

  describe("app calls", () => {
    const base = { schemaVersion: 2, slug: "x" };

    it("accepts callable and calls", () => {
      expect(
        messages({
          ...base,
          calls: { locations: ["lookup"], "b-2": ["a", "b"] },
          functions: { f: { route: false, callable: true } }
        })
      ).toEqual([]);
    });

    it("refuses a bad app slug, an empty list, a bad or repeated function name", () => {
      const found = errors({
        ...base,
        calls: { Bad_App: ["a"], empty: [], b: ["Bad", "ok", "ok"] }
      });
      expect(found.map((f) => f.path).sort()).toEqual(
        ["calls.Bad_App", "calls.b.0", "calls.b.2", "calls.empty"].sort()
      );
    });

    it("refuses more than 64 pairs", () => {
      const calls: Record<string, string[]> = {};
      for (let i = 0; i < 13; i++) calls[`app${i}`] = ["a", "b", "c", "d", "e"];
      const found = errors({ ...base, calls });
      expect(found.map((f) => f.path)).toContain("calls");
    });

    it("refuses a non-boolean callable", () => {
      expect(errors({ ...base, functions: { f: { callable: "yes" } } })).not.toHaveLength(0);
    });
  });

  it("warns, without refusing, about a field it does not know", () => {
    const issues = validateOxyAppManifest({
      schemaVersion: 2,
      slug: "x",
      colour: "blue",
      functions: { sync: { timeoutSecond: 30 } }
    });
    expect(issues.filter((i) => i.level === "error")).toEqual([]);
    expect(issues.map((i) => i.path)).toEqual(["colour", "functions.sync.timeoutSecond"]);
    expect(issues[1]?.message).toBe(
      'unknown field function "sync" `timeoutSecond`: the platform ignores what it does not read, so check the spelling against the schema'
    );
  });

  it("checks the app-level blocks", () => {
    expect(messages({ schemaVersion: 2, slug: "x", build: { outDir: 3 } })).toEqual([
      "`build.outDir` must be a string"
    ]);
    expect(messages({ schemaVersion: 2, slug: "x", migrations: {} })).toEqual([
      "`migrations.dir` is required"
    ]);
    // The server drops an unrecognised class with a warning and keeps the
    // prefix (`RetentionPolicy::from_rules`), so this is a warning here too.
    const retention = {
      schemaVersion: 2,
      slug: "x",
      storage: { retention: [{ prefix: "a", expireAfter: "2d" }] }
    };
    expect(errors(retention)).toEqual([]);
    expect(warnings(retention)).toEqual([
      {
        level: "warning",
        path: "storage.retention.0.expireAfter",
        message:
          '`storage.retention.0.expireAfter` must be "1d", "7d", "30d", "90d", "365d" or null — the platform drops this rule with a warning, so a broader sibling rule applies to the prefix, else it never expires'
      }
    ]);
    expect(
      messages({ schemaVersion: 2, slug: "x", storage: { retention: [{ expireAfter: "1d" }] } })
    ).toEqual(["`storage.retention.0.prefix` is required"]);
    expect(messages({ schemaVersion: 2, slug: "x", analytics: "no" })).toEqual([
      "`analytics` must be a boolean"
    ]);
    expect(messages({ schemaVersion: 2, slug: "x", env: { K: { required: "yes" } } })).toEqual([
      "`env.K.required` must be a boolean"
    ]);
  });

  it("keeps the notification catalogue's sentences where both speak", () => {
    const issues = validateOxyAppManifest({
      schemaVersion: 2,
      slug: "x",
      functions: { temps: { notify: { kinds: { temp_alarm: {} } } } },
      notifications: { kinds: { temp_alarm: { category: "finance", level: "urgent" } } }
    });
    expect(issues.map((i) => i.path)).toEqual([
      "notifications.kinds.temp_alarm.category",
      "notifications.kinds.temp_alarm.level"
    ]);
    expect(issues[1]?.message).toContain('"urgent"');
  });
});
