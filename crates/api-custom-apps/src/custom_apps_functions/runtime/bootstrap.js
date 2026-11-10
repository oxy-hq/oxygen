
class OxyResponse {
  constructor(body, init) {
    this.body = body ?? "";
    this.status = (init && init.status) || 200;
    this.headers = (init && init.headers) || {};
  }
  static json(value, init) {
    return new OxyResponse(JSON.stringify(value), {
      status: (init && init.status) || 200,
      headers: Object.assign({ "content-type": "application/json" }, init && init.headers),
    });
  }
}
globalThis.Response = OxyResponse;

// Base64. This isolate is bare deno_core — no `deno_web`, so none of the Web
// binary helpers exist, and V8 here predates `Uint8Array.prototype.toBase64`.
// Without these an author literally cannot produce the base64 that
// `ctx.email.send` attachments and `ctx.storage.put({encoding:"base64"})`
// require: `btoa` was simply `undefined`.
//
// These follow WHATWG semantics so that a helper unit-tested under Node behaves
// identically here. For BYTES use `bytesToBase64` from `@oxy-hq/sdk`, which is
// plain bundled JS and therefore the same function everywhere.
const __B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
// Reverse lookup. `__B64.indexOf(ch)` is a 64-char scan per input character;
// over the ~13.3 MB of base64 a 10 MiB attachment carries that is millions of
// scans inside a wall-clock-capped invocation.
const __B64R = new Uint8Array(256).fill(255);
for (let __i = 0; __i < 64; __i++) __B64R[__B64.charCodeAt(__i)] = __i;
// Accumulate into array segments rather than `out += c` per byte, which would
// allocate one rope node per byte at exactly the sizes this feature targets.
const __B64_CHUNK = 8192;

globalThis.btoa = (input) => {
  if (input instanceof ArrayBuffer || ArrayBuffer.isView(input)) {
    // The spec would ToString this to "37,80,68,70" and cheerfully encode the
    // wrong bytes. Refuse: a loud error beats a silently corrupt file, and the
    // named helper does the right thing.
    throw new TypeError(
      "btoa: expected a string. For bytes use bytesToBase64() from @oxy-hq/sdk"
    );
  }
  const s = String(input);
  const parts = [];
  let buf = "";
  for (let i = 0; i < s.length; i += 3) {
    const c0 = s.charCodeAt(i);
    const c1 = i + 1 < s.length ? s.charCodeAt(i + 1) : 0;
    const c2 = i + 2 < s.length ? s.charCodeAt(i + 2) : 0;
    if (c0 > 0xff || c1 > 0xff || c2 > 0xff) {
      // Same failure as a browser: btoa cannot carry UTF-8. Point at the way
      // out rather than emitting mojibake.
      throw new TypeError(
        "btoa: input contains characters outside the Latin1 range; for text " +
        "pass it directly with { encoding: \"utf8\" }"
      );
    }
    const n = (c0 << 16) | (c1 << 8) | c2;
    buf += __B64[(n >> 18) & 63] + __B64[(n >> 12) & 63]
      + (i + 1 < s.length ? __B64[(n >> 6) & 63] : "=")
      + (i + 2 < s.length ? __B64[n & 63] : "=");
    if (buf.length >= __B64_CHUNK) { parts.push(buf); buf = ""; }
  }
  parts.push(buf);
  return parts.join("");
};

globalThis.atob = (input) => {
  let s = String(input).replace(/[ \t\n\f\r]/g, "");
  // Strip padding BEFORE validating, and only when the length is a multiple of
  // 4 — that is what the spec does. Breaking out of the decode loop on the
  // first "=" instead would silently TRUNCATE: atob(chunkA + chunkB) where
  // chunkA ends in padding would return a short buffer and report success.
  if (s.length % 4 === 0) {
    let pad = 0;
    while (pad < 2 && s.charCodeAt(s.length - 1) === 61 /* = */) {
      s = s.slice(0, -1);
      pad++;
    }
  }
  if (s.indexOf("=") >= 0) {
    throw new TypeError("atob: '=' may only appear as trailing padding");
  }
  if (s.length % 4 === 1) throw new TypeError("atob: invalid base64 length");
  const parts = [];
  let chunk = [];
  let buf = 0;
  let bits = 0;
  for (let i = 0; i < s.length; i++) {
    const code = s.charCodeAt(i);
    const v = code < 256 ? __B64R[code] : 255;
    if (v === 255) throw new TypeError("atob: invalid base64 character '" + s[i] + "'");
    buf = (buf << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      chunk.push((buf >> bits) & 0xff);
      if (chunk.length >= __B64_CHUNK) {
        parts.push(String.fromCharCode.apply(null, chunk));
        chunk = [];
      }
    }
  }
  if (chunk.length) parts.push(String.fromCharCode.apply(null, chunk));
  return parts.join("");
};

// Wire the console developers reach for reflexively into the same host log
// sink as ctx.log — captured per-invocation and sent back with the response.
const __fmt = (a) => {
  if (typeof a === "string") return a;
  try {
    return JSON.stringify(a ?? null);
  } catch {
    // Circular refs / BigInt / etc. — native console.log tolerates these, so a
    // format failure must never fail the whole invocation.
    return String(a);
  }
};
const __log = (level) => (...args) => Deno.core.ops.op_ctx_log(level, args.map(__fmt).join(" "));
const __noop = () => {};
globalThis.console = {
  log: __log("info"),
  info: __log("info"),
  debug: __log("info"),
  warn: __log("warn"),
  error: __log("error"),
  trace: __log("info"),
  dir: __log("info"),
  assert: (cond, ...args) => {
    if (!cond) __log("error")("Assertion failed:", ...args);
  },
  // Extras stubbed to no-ops so an author reaching for them can't throw.
  table: __noop,
  group: __noop,
  groupCollapsed: __noop,
  groupEnd: __noop,
  count: __noop,
  countReset: __noop,
  time: __noop,
  timeEnd: __noop,
  timeLog: __noop,
};

function __wrapOp(opName) {
  return async (...args) => {
    const raw = await Deno.core.ops[opName](...args.map((a) =>
      typeof a === "string" ? a : JSON.stringify(a ?? null)
    ));
    const parsed = JSON.parse(raw);
    if (parsed && parsed.__oxyError) {
      const err = new Error(parsed.message);
      err.name = parsed.__oxyError;
      throw err;
    }
    return parsed;
  };
}

// The commit/rollback bracket behind ctx.tx and ctx.oltp.tx. It lives HERE
// rather than in the author's code on purpose: an author who forgets a rollback
// in a catch block leaves a transaction open holding locks, and the only
// reliable moment to close it is the one the runtime owns. Statements take bound
// parameters ($1, $2, …) — never build SQL by concatenating user input.
const __runTx = async (signature, label, beginOp, beginPayload, fn) => {
  if (typeof fn !== "function") {
    throw new TypeError(`${signature}: fn must be a function`);
  }
  const call = __wrapOp("op_ctx_tx");
  const { id } = await call(beginOp, beginPayload);
  let closed = false;
  // Every method re-checks `closed` so a handle that escapes the callback
  // (stashed on a global, captured by a stray promise) fails loudly instead of
  // addressing whatever transaction now holds that id.
  const live = (what) => {
    if (closed) {
      throw new Error(
        `${label}: this transaction is already finished — ${what} was called after the callback returned`,
      );
    }
  };
  const handle = {
    query: async (sql, params) => {
      live("query");
      // `??` not `||`: both map a real omission to [], but `||` also swallows 0
      // and "" — handing a wrong-typed argument to the host as "no parameters"
      // and producing a misleading arity error. Anything else is forwarded as-is
      // for the host to reject by name.
      const r = await call("query", { id, sql: String(sql), params: params ?? [] });
      return r.rows;
    },
    exec: async (sql, params) => {
      live("exec");
      const r = await call("exec", { id, sql: String(sql), params: params ?? [] });
      return r.rowCount;
    },
  };
  let result;
  try {
    result = await fn(handle);
  } catch (err) {
    closed = true;
    // Swallow a rollback failure: the connection drops either way, which the
    // server treats as a rollback, and surfacing it would replace the error the
    // author actually needs to see.
    try {
      await call("rollback", { id });
    } catch (_) {}
    throw err;
  }
  closed = true;
  await call("commit", { id });
  return result;
};

globalThis.__buildCtx = (ctxData) => ({
  // `org_id` is a back-compat mirror of `orgId`. The host used to serialize the
  // field snake_cased, so `ctx.user.orgId` — the name the SDK types and the docs
  // have always used — read `undefined`, and a tenant filter written against it
  // silently compared against nothing. The host now emits `orgId`; this keeps
  // any function written against the shipped `org_id` working.
  //
  // Removal is NOT gated on an SDK version. What reads this key is the function
  // source inside an already-published bundle, and a bundle keeps running until
  // someone republishes it — an SDK floor would never come due. The measurable
  // condition is the artifacts themselves: we hold every live build's
  // `functions/*.js` in the build store, so this is removable once no build a
  // live app points at contains `.org_id`.
  user: { ...ctxData.user, org_id: ctxData.user.orgId },
  env: ctxData.env,
  // The app environment this run is in. `environment` is the design's name;
  // `channel` the hold layer's. Same value: "production" or "staging".
  channel: ctxData.channel,
  environment: ctxData.channel,
  // Which published build is answering, in the shape the browser shell injects
  // as `window.__OXY_APP__` — so a record written here and one written in the
  // page about the same build carry the same id. A value, not an op: fixed for
  // the invocation and frozen so a handler cannot relabel its own records.
  app: Object.freeze({ buildId: ctxData.buildId, environment: ctxData.channel }),
  log: (...args) => Deno.core.ops.op_ctx_log("info", args.map(String).join(" ")),
  // Synchronous — pure CPU, so these skip the host-call channel entirely.
  //
  // verifyHmac is the one that matters: with req.headers carrying a signature,
  // an author would otherwise hand-roll HMAC in JS and compare with `===`,
  // which leaks the digest a byte at a time. Strip the provider's prefix
  // ("sha256=", "v0=") before calling — those formats are per-provider.
  crypto: {
    // `key` comes from configuration, never from the request, so an absent one
    // is an author error and throws. Coercing it would sign with the literal
    // "undefined" — a key anyone can guess.
    hmac: ({ algorithm, key, data, encoding }) => {
      if (key == null) throw new TypeError("ctx.crypto.hmac: `key` is required — is the secret set?");
      if (data == null) throw new TypeError("ctx.crypto.hmac: `data` is required");
      return Deno.core.ops.op_ctx_hmac(
        String(algorithm || "sha256"), String(key), String(data), String(encoding || "hex"));
    },
    verifyHmac: ({ algorithm, key, data, signature, encoding }) => {
      if (key == null) throw new TypeError("ctx.crypto.verifyHmac: `key` is required — is the secret set?");
      if (data == null) throw new TypeError("ctx.crypto.verifyHmac: `data` is required");
      // `signature` stays lenient on purpose: it is attacker-controlled, so an
      // absent or malformed one must reject rather than throw.
      return Deno.core.ops.op_ctx_verify_hmac(
        String(algorithm || "sha256"), String(key), String(data),
        String(signature ?? ""), String(encoding || "hex"));
    },
    // Both sides are symmetric here and either may be attacker-controlled (an
    // omitted header) or author-controlled (an unset secret) — we cannot tell
    // which. So an absent side is always `false`, never a throw: false fails
    // closed, and throwing would turn an omitted header into a 500.
    timingSafeEqual: (a, b) =>
      Deno.core.ops.op_ctx_timing_safe_equal(a == null ? "" : String(a), b == null ? "" : String(b)),
  },
  query: __wrapOp("op_ctx_query"),
  // queryStream(sql, opts?) — fetches up to FUNCTION_STREAM_MAX_ROWS rows in
  // one host call, then yields them to the caller in `opts.batchSize`-sized
  // arrays via an async generator. Not a true warehouse cursor (yet); see
  // design doc §11.5.
  queryStream: async function* (sql, opts) {
    const batchSize = (opts && opts.batchSize) || 1000;
    const rows = await __wrapOp("op_ctx_query_stream")(sql);
    for (let i = 0; i < rows.length; i += batchSize) {
      yield rows.slice(i, i + batchSize);
    }
  },
  fetch: __wrapOp("op_ctx_fetch"),
  warehouse: {
    // insert(database, table, rows) / exec(database, sql) /
    // upsert(database, table, rows, conflictColumns) — `op` stays a bare
    // string, the rest of the call is packed into a payload object that
    // __wrapOp JSON-stringifies.
    insert: (database, table, rows) =>
      __wrapOp("op_ctx_warehouse")("insert", { database, table, rows }),
    exec: (database, sql) =>
      __wrapOp("op_ctx_warehouse")("exec", { database, sql }),
    upsert: (database, table, rows, conflictColumns) =>
      __wrapOp("op_ctx_warehouse")("upsert", { database, table, rows, conflictColumns }),
    // query(database, sql) — a READ against a named database. `ctx.query` only
    // ever reaches the project default, so this is the one way an app reads its
    // own OLTP store. Not gated by `destinations`: that allowlist is about
    // writes, and postgres_managed resolves the read-only analyst regardless.
    query: (database, sql) =>
      __wrapOp("op_ctx_warehouse")("query", { database, sql }),
  },
  // tx(database, fn) — run `fn` inside one transaction on a pinned connection
  // to a declared destination. Commits when `fn` resolves, rolls back when it
  // throws, and rethrows the original error either way (see __runTx).
  tx: (database, fn) =>
    __runTx("ctx.tx(database, fn)", "ctx.tx", "begin", { database: String(database) }, fn),
  // oltp.query(sql, params?) / oltp.exec(sql, params?) — read/write the app's
  // OWN per-org OLTP schema (app_<writer>), and nothing else. This is the write
  // half ctx.warehouse cannot give an app on a managed database (that resolves
  // the read-only analyst, which also sees the org's raw_* extracts). The writer
  // is derived host-side from the app's own slug — oxy-app.json's
  // `oltp: { enabled }` only gates access, it never names the target — so no
  // database name crosses the boundary. Each call auto-commits (a failed
  // statement rolls back). Statements take bound parameters ($1, $2, …) — never
  // build SQL by concatenating user input.
  oltp: {
    // oltp.tx(fn) — several statements on the app's own schema as one
    // transaction: commits when `fn` resolves, rolls back when it throws. The
    // same bracket and handle as ctx.tx; the writer is derived host-side.
    tx: (fn) => __runTx("ctx.oltp.tx(fn)", "ctx.oltp.tx", "begin_oltp", {}, fn),
    query: async (sql, params) => {
      const r = await __wrapOp("op_ctx_oltp")("query", {
        sql: String(sql),
        params: params ?? [],
      });
      return r.rows;
    },
    exec: async (sql, params) => {
      const r = await __wrapOp("op_ctx_oltp")("exec", {
        sql: String(sql),
        params: params ?? [],
      });
      return r.rowCount;
    },
  },
  // airhouse.* — the app's own FACTS in its workspace's Airhouse: append-only
  // history (an order happened, a checklist was completed) in the schema
  // `app_<writer>`, derived host-side from the app's slug. Written as the app,
  // whoever invoked the function, so a schedule writes like a click. Reads may
  // name any schema; writes must name `${ctx.airhouse.schema}.<table>`, and
  // tables come from airhouseMigrations, not exec. DuckLake has no keys or
  // UNIQUE, so give each fact its source's id and keep one row per id on read.
  airhouse: {
    schema: ctxData.airhouseSchema ?? null,
    query: (sql) => __wrapOp("op_ctx_airhouse")("query", { sql: String(sql) }),
    exec: async (sql) => {
      await __wrapOp("op_ctx_airhouse")("exec", { sql: String(sql) });
    },
    append: async (table, rows) => {
      const r = await __wrapOp("op_ctx_airhouse")("append", { table: String(table), rows });
      return r.rowCount;
    },
  },
  secrets: {
    // set(key, value) — both stay bare strings so they arrive as the op's
    // `#[string]` args (matching op_ctx_airway_run's bare-string pipelineRef).
    set: (key, value) => __wrapOp("op_ctx_secrets_set")(String(key), String(value)),
  },
  email: {
    // send(input) — input is an object; __wrapOp JSON-stringifies it. The host
    // controls `from` (author sets replyTo only). Render templates to `html`
    // with `render` from @oxy-hq/sdk/email before calling this.
    send: (input) => __wrapOp("op_ctx_email_send")(input),
  },
  // notify(input) — tell people in this org something: { to, kind, title,
  // body?, link?, email?, slack?, idempotencyKey? }. `to` names people (a user
  // id, a role at a location, a team), never an address; the platform picks
  // inbox, push, email and Slack per person.
  notify: (input) => __wrapOp("op_ctx_notify")(input),
  org: {
    // people() — who is in this org, by name. Read-only, and NOT a contact
    // list: a display name and a role, never an email or a phone. No location —
    // the platform holds none for a member. Includes frontline workers granted
    // to THIS app, tagged `kind: "frontline"`; see the host handler for why the
    // scope is per-app.
    // Naming a colleague is a different need from being able to message them,
    // and only the first one was blocking anything.
    people: () => __wrapOp("op_ctx_org_people")(),
    // places() — the org's locations: hierarchy, status, timezone, and what
    // each integration calls the place. The whole registry; reach is applied
    // by the app, on top.
    places: () => __wrapOp("op_ctx_org_places")(),
    // assignments() — who holds which position where, for the people who can
    // reach this app. The roster, read.
    assignments: () => __wrapOp("op_ctx_org_assignments")(),
  },
  platform: {
    // facts({ weeks }) — ids and counts about every org on the deployment,
    // for an internal app. Two gates, both fail closed: the manifest's
    // `platform.facts`, and this app's org one of the deployment's internal
    // orgs (OXY_INTERNAL_ORGS). Never an email, a message or a row of customer
    // data.
    facts: (opts) => __wrapOp("op_ctx_platform_facts")(opts || {}),
  },
  storage: {
    // The app's asset store — uploaded files AND generated ones, one silo.
    // getUploadUrl/getDownloadUrl mint presigned URLs the BROWSER talks to
    // directly (bytes never cross this boundary); put/get/head/list/delete/copy
    // are server-side. `op` stays a bare string and the rest is packed into a
    // payload object that __wrapOp JSON-stringifies (matching ctx.warehouse).
    getUploadUrl: (opts) => __wrapOp("op_ctx_storage")("getUploadUrl", opts || {}),
    getDownloadUrl: (key, opts) =>
      __wrapOp("op_ctx_storage")("getDownloadUrl", Object.assign({ key: String(key) }, opts || {})),
    // put(pathname, body, opts) — body is a string; pass
    // { encoding: "base64" } for binary assets (PDF/PNG/Parquet).
    put: (pathname, body, opts) =>
      __wrapOp("op_ctx_storage")(
        "put",
        Object.assign({ pathname: String(pathname), body: String(body) }, opts || {})
      ),
    get: (key, opts) =>
      __wrapOp("op_ctx_storage")("get", Object.assign({ key: String(key) }, opts || {})),
    head: (key) => __wrapOp("op_ctx_storage")("head", { key: String(key) }),
    list: (opts) => __wrapOp("op_ctx_storage")("list", opts || {}),
    // delete(key) or delete([key, ...])
    delete: (keyOrKeys) =>
      __wrapOp("op_ctx_storage")(
        "delete",
        Array.isArray(keyOrKeys)
          ? { keys: keyOrKeys.map(String) }
          : { key: String(keyOrKeys) }
      ),
    copy: (fromKey, toPathname, opts) =>
      __wrapOp("op_ctx_storage")(
        "copy",
        Object.assign({ fromKey: String(fromKey), toPathname: String(toPathname) }, opts || {})
      ),
  },
  semantic: { query: __wrapOp("op_ctx_semantic_query") }, // spec is JSON-stringified by __wrapOp
  airway: {
    // run(pipelineRef, variables?) — pipelineRef stays a bare string,
    // variables is JSON-stringified by __wrapOp.
    run: (pipelineRef, variables) =>
      __wrapOp("op_ctx_airway_run")(String(pipelineRef), variables ?? null),
  },
});
