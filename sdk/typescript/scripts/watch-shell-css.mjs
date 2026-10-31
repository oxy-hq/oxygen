// Dev/watch runner for the SDK.
//
// tsdown's watcher only tracks its JS/TS module graph, so a standalone edit
// to src/shell/shell.css or src/motion/motion.css (copied assets, not bundled
// imports) never triggers a rebuild — dist would go stale during `dev`.
// This runner starts `tsdown --watch` AND watches both CSS files directly,
// re-copying one on change so live CSS edits are reflected immediately.
//
// The plain `pnpm build` (and every tsdown rebuild) still copies the CSS via
// `onSuccess` in tsdown.config.ts; this script only adds the CSS-only-edit
// case that tsdown can't see. Copies here are idempotent with that.
import { spawn } from "node:child_process";
import { copyFileSync, mkdirSync, watch } from "node:fs";
import { basename, dirname } from "node:path";

const ASSETS = ["src/shell/shell.css", "src/motion/motion.css"];

function copyCss(src) {
  const out = `dist/${basename(src)}`;
  try {
    mkdirSync(dirname(out), { recursive: true });
    copyFileSync(src, out);
    console.log(`[shell-css] ${src} -> ${out}`);
  } catch (err) {
    console.error(`[shell-css] copy failed: ${err.message}`);
  }
}

// Watch the directory (not the file) so the watch survives editors that save
// via atomic write-and-rename, which would orphan a file-level watch.
for (const src of ASSETS) {
  watch(dirname(src), { persistent: true }, (_event, filename) => {
    if (filename === basename(src)) copyCss(src);
  });
}

// tsdown owns the JS/TS build + its own onSuccess CSS copy; inherit its stdio
// so the usual watch output shows through.
const child = spawn("tsdown", ["--watch"], { stdio: "inherit", shell: true });

function shutdown(signal) {
  if (child.exitCode === null && !child.killed) child.kill(signal);
}
process.on("SIGINT", () => shutdown("SIGINT"));
process.on("SIGTERM", () => shutdown("SIGTERM"));
child.on("exit", (code) => process.exit(code ?? 0));
