/**
 * The credentials file is a contract with a program written in another
 * language. These tests are what make that claim checkable.
 */

import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  clearCredential,
  configDir,
  hostKey,
  loadCredential,
  loadToken,
  readStore,
  saveCredential
} from "./credentials.js";

let scratch: string;

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-cred-"));
  process.env.OXY_CREDENTIALS_PATH = join(scratch, "credentials.json");
  delete process.env.OXY_TOKEN;
});

afterEach(() => {
  delete process.env.OXY_CREDENTIALS_PATH;
  delete process.env.OXY_TOKEN;
});

describe("hostKey", () => {
  /**
   * Transcribed from `login.rs::tests::host_key_separates_envs`. If these two
   * ever disagree, each tool caches under a key the other cannot find and both
   * report the user as logged out while the file holds a valid token.
   */
  it("matches the Rust implementation case for case", () => {
    expect(hostKey("http://localhost:3000")).toBe("localhost:3000");
    expect(hostKey("https://app.oxygen-hq.com")).toBe("app.oxygen-hq.com");
    expect(hostKey("https://app-dev.oxygen-hq.com/")).toBe("app-dev.oxygen-hq.com");
  });

  it("keeps the port, because a laptop holds several localhost tokens", () => {
    expect(hostKey("http://localhost:5173")).not.toBe(hostKey("http://localhost:3000"));
  });

  it("falls back to the trimmed value when the target will not parse", () => {
    expect(hostKey("not a url/")).toBe("not a url");
  });
});

describe("configDir", () => {
  /**
   * THE BUG THIS PINS: the Rust doc comment says `~/.config/oxy/credentials.json`,
   * but it builds the path from `dirs::config_dir()`, which on macOS is
   * `~/Library/Application Support`. An implementation that believed the
   * comment would read an empty store on every Mac and report every developer
   * as logged out while `oxy` saw them logged in.
   */
  it("uses the platform rules of the Rust `dirs` crate, not the doc comment", () => {
    const dir = configDir();
    if (process.platform === "darwin") {
      expect(dir).toMatch(/Library\/Application Support$/);
      expect(dir).not.toMatch(/\.config$/);
    } else if (process.platform === "linux") {
      expect(dir).toMatch(/\.config$|^\//);
    }
  });
});

describe("the store", () => {
  it("round-trips the exact field names the Rust struct uses", () => {
    saveCredential("https://app.oxygen-hq.com", {
      token: "tok",
      email: "a@b.c",
      is_app_admin: true
    });
    const raw = JSON.parse(readFileSync(process.env.OXY_CREDENTIALS_PATH as string, "utf8"));
    expect(Object.keys(raw)).toEqual(["app.oxygen-hq.com"]);
    // Snake case, and no envelope: the Rust serialises a bare
    // `HashMap<String, HostCredential>`.
    expect(Object.keys(raw["app.oxygen-hq.com"]).sort()).toEqual([
      "email",
      "is_app_admin",
      "token"
    ]);
  });

  it("reads a file the Rust binary wrote", () => {
    writeFileSync(
      process.env.OXY_CREDENTIALS_PATH as string,
      JSON.stringify({
        "app.oxygen-hq.com": { token: "from-rust", email: "x@y.z", is_app_admin: false }
      })
    );
    expect(loadToken("https://app.oxygen-hq.com")).toBe("from-rust");
  });

  it("leaves other hosts alone when one is saved", () => {
    saveCredential("https://a.example.com", { token: "1", email: "", is_app_admin: false });
    saveCredential("https://b.example.com", { token: "2", email: "", is_app_admin: false });
    expect(Object.keys(readStore()).sort()).toEqual(["a.example.com", "b.example.com"]);
  });

  /**
   * A corrupt file must not throw: a missing one is the ordinary state on a
   * fresh machine, and "not authenticated, run oxyc login" is the right answer
   * to both.
   */
  it("degrades to empty rather than throwing on a corrupt file", () => {
    writeFileSync(process.env.OXY_CREDENTIALS_PATH as string, "{ not json");
    expect(readStore()).toEqual({});
    expect(loadToken("https://app.oxygen-hq.com")).toBeUndefined();
  });

  it("treats an empty token as absent", () => {
    saveCredential("https://a.example.com", { token: "   ", email: "", is_app_admin: false });
    expect(loadToken("https://a.example.com")).toBeUndefined();
  });
});

/**
 * `token_id` and `expires_at` arrived with the PKCE login. They are additive in
 * both directions, and each direction is somebody's existing login: a file
 * written before them must still load, and a file carrying them must survive
 * being rewritten by a login to some OTHER host.
 */
describe("the optional token fields", () => {
  it("loads a file written before they existed, and reads them as absent", () => {
    writeFileSync(
      process.env.OXY_CREDENTIALS_PATH as string,
      JSON.stringify({
        "app.oxygen-hq.com": { token: "eyJ.session.jwt", email: "x@y.z", is_app_admin: true }
      })
    );
    const cached = loadCredential("https://app.oxygen-hq.com");
    expect(cached).toEqual({ token: "eyJ.session.jwt", email: "x@y.z", is_app_admin: true });
    expect(cached?.token_id).toBeUndefined();
    expect(cached?.expires_at).toBeUndefined();
  });

  it("round-trips them, under the same snake-case names the server uses", () => {
    saveCredential("https://app.oxygen-hq.com", {
      token: "oxy_pat_abc",
      email: "a@b.c",
      is_app_admin: false,
      token_id: "tok-1",
      expires_at: "2026-12-30T00:00:00Z"
    });
    const raw = JSON.parse(readFileSync(process.env.OXY_CREDENTIALS_PATH as string, "utf8"));
    expect(raw["app.oxygen-hq.com"]).toEqual({
      token: "oxy_pat_abc",
      email: "a@b.c",
      is_app_admin: false,
      token_id: "tok-1",
      expires_at: "2026-12-30T00:00:00Z"
    });
  });

  it("keeps an older host's entry byte-for-byte when a newer one is saved beside it", () => {
    const old = { token: "eyJ.session.jwt", email: "x@y.z", is_app_admin: true };
    writeFileSync(
      process.env.OXY_CREDENTIALS_PATH as string,
      JSON.stringify({ "aip.dev.oxy.tech": old })
    );
    saveCredential("https://app.oxygen-hq.com", {
      token: "oxy_pat_abc",
      email: "x@y.z",
      is_app_admin: true,
      token_id: "tok-1"
    });
    expect(readStore()["aip.dev.oxy.tech"]).toEqual(old);
    expect(loadToken("https://aip.dev.oxy.tech")).toBe("eyJ.session.jwt");
  });

  it("drops one host on logout and leaves the rest", () => {
    saveCredential("https://a.example.com", { token: "1", email: "", is_app_admin: false });
    saveCredential("https://b.example.com", { token: "2", email: "", is_app_admin: false });
    expect(clearCredential("https://a.example.com")).toBe(true);
    expect(clearCredential("https://a.example.com")).toBe(false);
    expect(Object.keys(readStore())).toEqual(["b.example.com"]);
  });
});
