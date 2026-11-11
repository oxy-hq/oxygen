import { fileURLToPath } from "node:url";
import { defineConfig } from "tsdown";

export default defineConfig({
  entry: ["src/main.ts"],
  format: ["esm"],
  dts: false,
  sourcemap: true,
  clean: true,
  minify: false,
  // **This package is public; the repository it is built from is not.** Both of
  // these keep the tarball from carrying the source it was built out of, and
  // neither is cosmetic:
  //
  // * `sourcemapExcludeSources` drops `sourcesContent`. With it on, the map
  //   still maps a stack frame to a file and a line, which is what a bug report
  //   needs; with it off, `dist/main.mjs.map` embedded all 159 source files
  //   verbatim, so `npm pack @oxy-hq/cli` handed anyone the whole CLI source.
  // * `comments: false` strips the bundle's comments. `minify` is off on
  //   purpose — a readable bundle is worth keeping — but that left every
  //   developer note in the shipped file, including ~15 Rust source paths and
  //   ~10 `internal-docs/` filenames that mean nothing to a reader outside
  //   this repo and say more than they should.
  //
  // Together they took the build from 4.04 MB to 1.74 MB. Comments in `src/`
  // are unaffected: say what you like there, it does not ship.
  outputOptions: {
    sourcemapExcludeSources: true,
    comments: false
  },
  outDir: "dist",
  target: "node20",
  // The SDK's manifest contract is bundled from its SOURCE, so `dist/main.mjs`
  // and the standalone binaries carry it and a published `oxyc` depends on no
  // SDK package at run time. Same mapping as `tsconfig.json` `paths`.
  alias: {
    "@oxy-hq/sdk/manifest": fileURLToPath(
      new URL("../typescript/src/manifest.ts", import.meta.url)
    ),
    // `oxyc fn dev` runs a handler on the SDK's testing context, bundled the same way.
    "@oxy-hq/sdk/testing": fileURLToPath(
      new URL("../typescript/src/testing/index.ts", import.meta.url)
    )
  },
  // `npx @oxy-hq/cli` and a global install both exec this file directly.
  banner: {
    js: "#!/usr/bin/env node"
  }
});
