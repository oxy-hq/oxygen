import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { brotliDecompressSync } from "node:zlib";
import { beforeEach, describe, expect, it } from "vitest";
import { PRECOMPRESSED_SUFFIX, precompressDir } from "./precompress";

let out: string;

const write = async (name: string, contents: string | Buffer) => {
  const path = join(out, name);
  await mkdir(join(path, ".."), { recursive: true });
  await writeFile(path, contents);
  return path;
};

/** Text that actually compresses, unlike a short random string. */
const compressible = (n: number) => "const a = 1; export default a;\n".repeat(n);

beforeEach(async () => {
  out = await mkdtemp(join(tmpdir(), "oxy-precompress-"));
});

describe("precompressDir", () => {
  it("writes a decompressible sibling for a compressible asset", async () => {
    const source = compressible(200);
    await write("assets/main-a1b2.js", source);

    const result = await precompressDir(out);

    expect(result.files).toBe(1);
    const sibling = join(out, "assets/main-a1b2.js" + PRECOMPRESSED_SUFFIX);
    expect(existsSync(sibling)).toBe(true);
    expect(brotliDecompressSync(await readFile(sibling)).toString()).toBe(source);
    expect(result.compressedBytes).toBeLessThan(result.rawBytes);
  });

  /**
   * The server splices `window.__OXY_ORG__` into the shell per org subdomain
   * and stamps an ETag over the result (`finalize_html` in
   * `crates/server/src/commands/serve.rs`), so the bytes on the wire are never
   * the bytes on disk. A sibling for it would be stale by construction, and
   * would be served to an org subdomain with another org's context in it.
   */
  it("never compresses HTML", async () => {
    await write("index.html", `<!doctype html>${compressible(200)}`);

    const result = await precompressDir(out);

    expect(result.files).toBe(0);
    expect(existsSync(join(out, "index.html" + PRECOMPRESSED_SUFFIX))).toBe(false);
  });

  it("skips containers that are already compressed", async () => {
    // woff2 is brotli; a png is deflate. Both would grow.
    await write("fonts/Inter.woff2", Buffer.alloc(200_000, 7));
    await write("icons/logo.png", Buffer.alloc(200_000, 7));

    const result = await precompressDir(out);

    expect(result.files).toBe(0);
  });

  it("skips files too small to be worth a second object", async () => {
    await write("assets/tiny.js", "export const a=1;");

    expect((await precompressDir(out)).files).toBe(0);
  });

  it("leaves no sibling for a file that does not compress", async () => {
    // Incompressible bytes in a .js file — an inlined encrypted or base64
    // blob behaves like this. A sibling here would add binary size for
    // nothing. Fixed-seed xorshift32 rather than randomBytes so the test
    // asserts the same input on every run.
    const noise = Buffer.alloc(64 * 1024);
    let state = 0x9e3779b9;
    for (let i = 0; i < noise.length; i++) {
      state ^= state << 13;
      state ^= state >>> 17;
      state ^= state << 5;
      noise[i] = state & 0xff;
    }
    await write("assets/blob.js", noise);

    expect((await precompressDir(out)).files).toBe(0);
  });

  it("does not compress its own output on a second run", async () => {
    await write("assets/main.js", compressible(200));

    const first = await precompressDir(out);
    const second = await precompressDir(out);

    expect(first.files).toBe(1);
    expect(second.files).toBe(1);
    expect(
      existsSync(join(out, `assets/main.js${PRECOMPRESSED_SUFFIX}${PRECOMPRESSED_SUFFIX}`))
    ).toBe(false);
  });

  it("reaches files copied from publicDir, not just bundle output", async () => {
    // `closeBundle` rather than `generateBundle` exists for exactly this: the
    // 0.83 MiB variable font never enters Rollup's graph.
    await write("fonts/Inter-VariableFont.ttf", compressible(4000));

    expect((await precompressDir(out)).files).toBe(1);
  });
});
