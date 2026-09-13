#!/usr/bin/env node
// Capture the HQ launcher-card image for this app into `public/card.webp`.
//
// The card is the 1280x640 image the Oxy HQ home shows for your app
// (manifest `art` field). This script boots the Vite dev server, opens the
// app in a headless browser, waits for it to render, and writes a
// screenshot to `public/card.webp` — which Vite copies to the bundle root, so
// it serves at `/customer-apps/<org>/<slug>/card.webp` (the URL the server
// derives from a RELATIVE `art: "card.webp"`). Never hardcode the base path
// into `art`; keep it relative and let the plugin place it.
//
// WebP at quality 0.8, not PNG: the home page downloads every card's image on
// every visit. A 1280x640 dashboard screenshot is ~290 KB as PNG and 40-70 KB
// as WebP, and `oxyc publish` warns about an `art` file over 200 KB.
// Playwright only captures PNG/JPEG, so the PNG is re-encoded by the same
// headless Chromium (canvas.toDataURL) — no image library needed.
//
// Usage:
//   pnpm run screenshot                 # dev server, default readiness wait
//   pnpm run screenshot -- --url http://localhost:5173/   # an already-running server
//   pnpm run screenshot -- --wait "[data-oxy-card-ready]" # wait for a specific element
//   pnpm run screenshot -- --selector "main"              # crop to one element
//   pnpm run screenshot -- --settle 2500                  # extra ms after ready
//
// After it writes public/card.webp, set `"art": "card.webp"` in oxy-app.json,
// rebuild, and `oxyc publish`.
//
// Playwright is invoked on demand — it is NOT a default dependency of the
// scaffold. If it is missing the script prints the one-liner to add it.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const CARD_WIDTH = 1280;
const CARD_HEIGHT = 640;
const OUT_PATH = path.resolve(process.cwd(), "public", "card.webp");
const WEBP_QUALITY = 0.8;
const DEV_PORT = 5173;

function parseArgs(argv) {
  const opts = {
    url: null, // when set, screenshot a server we don't manage
    wait: "#root > *", // default: app mounted something under #root
    selector: null, // crop to this element instead of the 1280x640 viewport
    settle: 1200, // extra ms after ready, for charts/fonts to paint
    port: DEV_PORT,
  };
  // Parse a numeric flag, erroring on a typo instead of silently yielding NaN
  // (which would no-op the settle wait / pick a bogus port).
  const num = (name, raw) => {
    const n = Number(raw);
    if (!Number.isFinite(n)) {
      console.error(`[screenshot] --${name} expects a number, got "${raw}"`);
      process.exit(1);
    }
    return n;
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const val = () => argv[++i];
    if (a === "--url") opts.url = val();
    else if (a === "--wait") opts.wait = val();
    else if (a === "--selector") opts.selector = val();
    else if (a === "--settle") opts.settle = num("settle", val());
    else if (a === "--port") opts.port = num("port", val());
    else if (a === "-h" || a === "--help") {
      console.log(
        "Usage: pnpm run screenshot [-- --url <u>] [--wait <sel>] [--selector <sel>] [--settle <ms>] [--port <n>]",
      );
      process.exit(0);
    }
  }
  return opts;
}

async function loadPlaywright() {
  try {
    const { chromium } = await import("playwright");
    return chromium;
  } catch {
    console.error(
      "\n[screenshot] Playwright isn't installed. Add it on demand:\n" +
        "    pnpm add -D playwright && pnpm exec playwright install chromium\n" +
        "  (it is intentionally not a default scaffold dependency).\n",
    );
    process.exit(1);
  }
}

// Spawn `pnpm dev` and resolve once the server answers. Returns a handle
// with a stop() that kills the whole process group.
function startDevServer(port) {
  const url = `http://localhost:${port}/`;
  const child = spawn("pnpm", ["dev", "--port", String(port), "--strictPort"], {
    stdio: ["ignore", "pipe", "inherit"],
    env: process.env,
    detached: true,
  });
  child.stdout.on("data", (b) => process.stdout.write(`[dev] ${b}`));
  const stop = () => {
    try {
      if (process.platform === "win32") {
        // Windows has no POSIX process groups; kill the child directly.
        // Vite's subtree may linger — acceptable for a local dev helper.
        child.kill();
      } else {
        // Negative pid → kill the group (Vite + its children).
        process.kill(-child.pid, "SIGTERM");
      }
    } catch {
      /* already gone */
    }
  };
  return { url, child, stop };
}

async function waitForServer(url, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url);
      if (res.ok) return;
    } catch {
      /* not up yet */
    }
    await new Promise((r) => setTimeout(r, 400));
  }
  throw new Error(`dev server never became ready at ${url}`);
}

// Re-encode a PNG screenshot as WebP inside headless Chromium. A blank page,
// not the app's, so the app's CSP can't block the data: image.
async function pngToWebp(browser, png) {
  const page = await browser.newPage();
  try {
    const dataUrl = await page.evaluate(
      async ({ b64, quality }) => {
        const img = new Image();
        img.src = `data:image/png;base64,${b64}`;
        await img.decode();
        const canvas = document.createElement("canvas");
        canvas.width = img.naturalWidth;
        canvas.height = img.naturalHeight;
        canvas.getContext("2d").drawImage(img, 0, 0);
        return canvas.toDataURL("image/webp", quality);
      },
      { b64: png.toString("base64"), quality: WEBP_QUALITY },
    );
    // A browser without a WebP encoder silently falls back to PNG.
    if (!dataUrl.startsWith("data:image/webp")) {
      throw new Error("this Chromium cannot encode WebP");
    }
    return Buffer.from(dataUrl.slice(dataUrl.indexOf(",") + 1), "base64");
  } finally {
    await page.close();
  }
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const chromium = await loadPlaywright();

  // Wrap everything from the dev-server spawn onward so `finally` always
  // tears the (detached) server group down — even if the server never
  // becomes ready or the browser fails to launch.
  let server = null;
  let browser = null;
  try {
    let targetUrl = opts.url;
    if (!targetUrl) {
      server = startDevServer(opts.port);
      targetUrl = server.url;
      console.log(`[screenshot] starting dev server on ${targetUrl} …`);
      await waitForServer(targetUrl);
    } else {
      console.log(`[screenshot] using running server ${targetUrl}`);
    }

    browser = await chromium.launch();
    const page = await browser.newPage({
      viewport: { width: CARD_WIDTH, height: CARD_HEIGHT },
      deviceScaleFactor: 1, // exact 1280x640 px, no HiDPI upscale
    });
    await page.goto(targetUrl, { waitUntil: "networkidle", timeout: 30_000 });
    if (opts.wait) {
      await page.waitForSelector(opts.wait, { state: "visible", timeout: 30_000 });
    }
    if (opts.settle > 0) await page.waitForTimeout(opts.settle);

    let png;
    if (opts.selector) {
      const el = await page.$(opts.selector);
      if (!el) throw new Error(`--selector "${opts.selector}" matched nothing`);
      png = await el.screenshot();
    } else {
      // clip to the exact card frame so the file is always 1280x640
      png = await page.screenshot({
        clip: { x: 0, y: 0, width: CARD_WIDTH, height: CARD_HEIGHT },
      });
    }
    const webp = await pngToWebp(browser, png);

    if (!existsSync(path.dirname(OUT_PATH))) {
      mkdirSync(path.dirname(OUT_PATH), { recursive: true });
    }
    writeFileSync(OUT_PATH, webp);
    console.log(`[screenshot] wrote ${OUT_PATH} (${Math.round(webp.length / 1024)} KB)`);
    console.log('[screenshot] set  "art": "card.webp"  in oxy-app.json, then `oxyc publish`.');
  } finally {
    if (browser) await browser.close();
    if (server) server.stop();
  }
}

main().catch((err) => {
  console.error(`[screenshot] failed: ${err?.message ?? err}`);
  process.exit(1);
});
