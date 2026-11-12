/**
 * The Claude Code cloud-session hook, as the template ships it.
 *
 * Two copies of one script: oxygen-internal's `.claude/hooks/` and the
 * template's, which `oxyc update` rewrites in every customer repo. A fix made
 * to one and not the other is a cloud session that cannot publish in half the
 * repos, so they are held byte-identical here.
 */

import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { ExitCode } from "../util/errors.js";
import { loadManifest, roleFor, scopeFor, unclassified } from "./manifest.js";
import { repoPathFor, walkFiles } from "./render.js";

const HOOK = ".claude/hooks/oxyc-cloud-session.sh";
const SETTINGS = ".claude/settings.json";
const template = fileURLToPath(new URL("../../template/", import.meta.url));
const repoRoot = fileURLToPath(new URL("../../../../", import.meta.url));
const BIN = fileURLToPath(new URL("../../dist/main.mjs", import.meta.url));

describe("the cloud-session hook", () => {
  it("is the same file here and in the template", () => {
    expect(readFileSync(`${template}${HOOK}`, "utf8")).toBe(
      readFileSync(`${repoRoot}${HOOK}`, "utf8")
    );
  });

  it("is executable in the template", () => {
    expect(statSync(`${template}${HOOK}`).mode & 0o111).not.toBe(0);
  });

  it("is what the template's settings run at session start", () => {
    const settings = JSON.parse(readFileSync(`${template}${SETTINGS}`, "utf8"));
    const commands: string[] = settings.hooks.SessionStart.flatMap(
      (entry: { hooks: { command: string }[] }) => entry.hooks.map((h) => h.command)
    );
    expect(commands.some((c) => c.includes(HOOK))).toBe(true);
  });

  it("is classified: the hook managed, the settings mixed, both at the repo root", () => {
    const manifest = loadManifest(`${template}.oxyc-managed`);
    expect(roleFor(manifest, HOOK)).toBe("managed");
    expect(roleFor(manifest, SETTINGS)).toBe("mixed");
    expect(scopeFor(manifest, HOOK)).toBe("repo");
    expect(scopeFor(manifest, SETTINGS)).toBe("repo");
  });

  /**
   * The mint line the hook prints is what an agent runs verbatim. It once
   * asked for `--standing`, which `--no-browser` refuses (exit 2), so every
   * cloud session that followed it minted nothing. Run the printed flags
   * through the real CLI: anything but a usage error got past the parser.
   */
  it("prints a mint command the CLI accepts", () => {
    const home = mkdtempSync(join(tmpdir(), "oxyc-hook-"));
    const bin = join(home, "bin");
    mkdirSync(bin);
    // `command -v oxyc` must succeed, or the hook would `npm install -g`.
    writeFileSync(join(bin, "oxyc"), "#!/bin/sh\n", { mode: 0o755 });
    // A URL env: the hook probes no host for it, and the mint fails fast.
    const target = "http://127.0.0.1:9";
    const hook = spawnSync("bash", [`${template}${HOOK}`], {
      encoding: "utf8",
      input: "{}",
      env: {
        PATH: `${bin}:${process.env.PATH}`,
        HOME: home,
        CLAUDE_CODE_REMOTE: "true",
        OXY_ENV: target
      }
    });
    rmSync(home, { recursive: true, force: true });
    expect(hook.status).toBe(0);
    const line = hook.stdout.split("\n").find((l) => l.includes("oxyc tokens create"));
    const flags = line
      ?.match(/oxyc (tokens create [^>)]*)/)?.[1]
      ?.trim()
      .split(/\s+/);
    expect(flags, hook.stdout).toBeDefined();
    expect(flags).toContain(target);

    const mint = spawnSync(process.execPath, [BIN, ...(flags ?? [])], {
      encoding: "utf8",
      timeout: 30_000,
      env: { ...process.env, OXY_TOKEN: "", OXY_APPROVER: "", NO_COLOR: "1" }
    });
    expect(mint.status, mint.stderr).not.toBe(ExitCode.USAGE);
  });

  /**
   * `OXY_TOKEN=from-proxy`: the network secret is the credential, so the hook
   * prints no mint line and checks the secret is attached with `oxyc whoami`.
   * `oxyc` is a stub here that exits with the code a real whoami would.
   */
  const runWithProxy = (whoamiExit: number, tokenFile = false) => {
    const home = mkdtempSync(join(tmpdir(), "oxyc-hook-"));
    const bin = join(home, "bin");
    mkdirSync(bin);
    writeFileSync(join(bin, "oxyc"), `#!/bin/sh\nexit ${whoamiExit}\n`, { mode: 0o755 });
    if (tokenFile) {
      mkdirSync(join(home, ".config/oxy"), { recursive: true });
      writeFileSync(join(home, ".config/oxy/agent-token.env"), "export OXY_TOKEN=oxy_pat_x\n");
    }
    const hook = spawnSync("bash", [`${template}${HOOK}`], {
      encoding: "utf8",
      input: "{}",
      env: {
        PATH: `${bin}:${process.env.PATH}`,
        HOME: home,
        CLAUDE_CODE_REMOTE: "true",
        OXY_ENV: "http://127.0.0.1:9",
        OXY_TOKEN: "from-proxy"
      }
    });
    rmSync(home, { recursive: true, force: true });
    expect(hook.status).toBe(0);
    expect(hook.stdout).not.toContain("oxyc tokens create");
    return hook.stdout;
  };

  it("names the network secret when OXY_TOKEN=from-proxy and whoami works", () => {
    expect(runWithProxy(0)).toContain("credential is the environment's network secret");
  });

  it("says the network secret is not attached when whoami fails", () => {
    const out = runWithProxy(ExitCode.AUTH);
    expect(out).toContain("'oxyc whoami --env http://127.0.0.1:9' fails");
    expect(out).toContain("Allowed websites");
  });

  it("says a minted token file replaces the sentinel", () => {
    expect(runWithProxy(0, true)).toContain("replaces it in every command");
  });

  it("leaves no file the template ships unclassified", () => {
    const manifest = loadManifest(`${template}.oxyc-managed`);
    const paths = walkFiles(template).map(repoPathFor);
    expect(unclassified(manifest, paths)).toEqual({ missingRole: [], missingScope: [] });
  });
});
