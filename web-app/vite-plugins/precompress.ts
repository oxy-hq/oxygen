/**
 * Build-time brotli pre-compression for `dist/`.
 *
 * The app's own assets go out of a `serve` pod uncompressed: the only
 * `CompressionLayer` in the tree sits on the `/customer-apps/{*path}` mount
 * (`crates/server/src/custom_apps_seams.rs`), and the SPA's static handler
 * (`crates/server/src/commands/serve.rs`) has none. A first visit therefore
 * downloads the full ~7.5 MB of JS and CSS.
 *
 * This is the same trade the custom-app bundles already make
 * (`crates/custom-apps/src/precompress.rs`): compress once per build rather
 * than once per request per replica. A content-hashed `index-a1b2c3.js` has
 * exactly one set of bytes for its whole lifetime, so compressing it on each
 * request is pure CPU burn — and here the output is embedded by `include_dir!`
 * into the binary, so there is no store round trip either. The Rust side probes
 * for the `.br` sibling and streams it (`crates/server/src/static_precompressed.rs`).
 *
 * ## Brotli only, no gzip sibling
 *
 * `build.target` is `es2020`, which needs Chrome 80 / Firefox 72 / Safari 13.1.
 * Brotli landed in Chrome 50, Firefox 44 and Safari 11, so every browser that
 * can execute this bundle advertises `br`. A gzip sibling would add bytes to
 * every binary for a client that cannot run the code it would decompress. A
 * client that sends neither (an old `curl`, a proxy that strips the header,
 * Chrome on plain-HTTP `localhost`) gets the uncompressed original, which is
 * the behaviour this plugin is replacing.
 *
 * ## What is deliberately not compressed
 *
 * - **`index.html`.** The server injects `window.__OXY_ORG__` into it per org
 *   subdomain and stamps an ETag over the result (`finalize_html`), so the
 *   bytes on the wire are not the bytes on disk. A `.br` sibling would be
 *   stale by construction. It is 3.7 KB.
 * - **Already-compressed containers** (png/jpg/gif/webp/woff/woff2/wasm).
 *   Brotli over an entropy-coded file spends CPU to grow it.
 * - **Files below `MIN_PRECOMPRESS_BYTES`**, where the sibling costs more
 *   binary than the transfer saves.
 */

import { readdir, readFile, writeFile } from "node:fs/promises";
import { extname, join } from "node:path";
import { promisify } from "node:util";
import { brotliCompress, constants as zlibConstants } from "node:zlib";
import type { Plugin } from "vite";

const compress = promisify(brotliCompress);

/** Suffix for the pre-compressed sibling. `assets/a.js` → `assets/a.js.br`. */
export const PRECOMPRESSED_SUFFIX = ".br";

/**
 * Below this, the sibling adds more to the binary than it removes from the
 * wire. Roughly one TCP segment, matching `crates/custom-apps/src/precompress.rs`.
 */
const MIN_PRECOMPRESS_BYTES = 1024;

/**
 * Text-shaped build output. `.html` is excluded on purpose — see the module
 * docs. `.ttf` is in because a variable font is uncompressed (the 0.83 MiB
 * `Inter-VariableFont.ttf`); `.woff2` is not, because woff2 *is* brotli.
 *
 * `.map` is out: sourcemaps are emitted only in a build that uploads them to
 * Sentry, which then deletes them from `dist/`. A sibling for a file that is
 * about to be removed is an orphan the binary would carry forever.
 */
const COMPRESSIBLE_EXTENSIONS = new Set([
  ".js",
  ".mjs",
  ".cjs",
  ".css",
  ".json",
  ".webmanifest",
  ".svg",
  ".txt",
  ".ttf",
  ".ico"
]);

/**
 * Quality 11 is the right call *here* even though the custom-app path uses 9.
 * There it runs inside `oxyc publish`, an interactive command an engineer
 * waits on. This runs in a production build that already takes minutes, and
 * its output is copied into every binary and every CDN edge — pay once,
 * benefit on every first visit forever.
 */
const BROTLI_QUALITY = 11;

/** A file is only worth a sibling if the sibling is meaningfully smaller. */
const MIN_RATIO_GAIN = 0.95;

async function* walk(dir: string): AsyncGenerator<string> {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) yield* walk(full);
    else if (entry.isFile()) yield full;
  }
}

export interface PrecompressResult {
  files: number;
  rawBytes: number;
  compressedBytes: number;
}

/**
 * Compress every eligible file under `outDir`, writing `<path>.br` siblings.
 * Exported separately from the plugin so it is testable without running Vite.
 */
export async function precompressDir(outDir: string): Promise<PrecompressResult> {
  const candidates: string[] = [];
  for await (const file of walk(outDir)) {
    if (file.endsWith(PRECOMPRESSED_SUFFIX)) continue;
    if (!COMPRESSIBLE_EXTENSIONS.has(extname(file).toLowerCase())) continue;
    candidates.push(file);
  }

  const result: PrecompressResult = { files: 0, rawBytes: 0, compressedBytes: 0 };

  await Promise.all(
    candidates.map(async (file) => {
      const raw = await readFile(file);
      if (raw.byteLength < MIN_PRECOMPRESS_BYTES) return;

      const compressed = await compress(raw, {
        params: {
          [zlibConstants.BROTLI_PARAM_QUALITY]: BROTLI_QUALITY,
          [zlibConstants.BROTLI_PARAM_SIZE_HINT]: raw.byteLength
        }
      });

      // A file that barely compresses (an inlined data: URI, a pre-minified
      // blob) would cost binary size for nothing.
      if (compressed.byteLength > raw.byteLength * MIN_RATIO_GAIN) return;

      await writeFile(file + PRECOMPRESSED_SUFFIX, compressed);
      result.files += 1;
      result.rawBytes += raw.byteLength;
      result.compressedBytes += compressed.byteLength;
    })
  );

  return result;
}

/**
 * Record what was compressed, so a human reading a build log — or a reviewer
 * asking "did this actually do anything" — gets the number rather than a
 * promise of one.
 */
const mib = (bytes: number) => (bytes / 1024 / 1024).toFixed(2);

export const precompress = (): Plugin => {
  let outDir = "dist";
  return {
    name: "oxy-precompress",
    apply: "build",
    configResolved(config) {
      outDir = config.build.outDir;
    },
    // `closeBundle`, not `generateBundle`: files copied from `publicDir`
    // (fonts, the service worker, the webmanifest) never enter the Rollup
    // bundle graph, and they are exactly the large static files worth
    // compressing.
    async closeBundle() {
      const { files, rawBytes, compressedBytes } = await precompressDir(outDir);
      if (files === 0) return;
      this.info(
        `brotli: ${files} files, ${mib(rawBytes)} MiB → ${mib(compressedBytes)} MiB ` +
          `(${((1 - compressedBytes / rawBytes) * 100).toFixed(0)}% smaller on the wire)`
      );
    }
  };
};
