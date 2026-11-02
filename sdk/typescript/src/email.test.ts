// `@oxy-hq/sdk/email` is the one SDK entry that runs inside the Oxy Functions
// isolate, which is V8 with almost nothing on `globalThis`: no timers, no
// `queueMicrotask`, no `TextEncoder`, `Buffer` or `process` (bootstrap.js in
// crates/api-custom-apps adds `Response`, `btoa`/`atob` and `console`, and
// that is all). preact and preact-render-to-string are bundled into the
// function, so a major bump of either is exactly the change that could start
// reaching for one of those — and nothing else in CI would notice, because the
// SDK's other tests run on Node, where every one of them exists.
//
// So the last test bundles the renderer the way `oxyc publish` does
// (`--bundle --format=esm --platform=neutral`, sdk/cli/src/publish/functions.ts)
// and evaluates it in a bare V8 context: stricter than the isolate, never
// looser. The others pin what a template author sees in the HTML.

import { fileURLToPath } from "node:url";
import vm from "node:vm";
import { build } from "esbuild";
import { type ComponentChildren, h } from "preact";
import { describe, expect, it } from "vitest";
import { render } from "./email";

function Card({ title, children }: { title: string; children?: ComponentChildren }) {
  return h(
    "div",
    { style: { padding: 24, width: 480, lineHeight: 1.5, opacity: 0.9, fontWeight: 700 } },
    h("h1", null, title),
    children
  );
}

describe("render", () => {
  it("renders a component with its props to an HTML string", () => {
    expect(render(Card, { title: "Welcome, Ada!" })).toBe(
      '<div style="padding:24px;width:480px;line-height:1.5;opacity:0.9;font-weight:700;">' +
        "<h1>Welcome, Ada!</h1></div>"
    );
  });

  // preact 11 stopped appending `px` to numeric styles in core (it moved to
  // preact/compat). Email HTML is rendered by preact-render-to-string, which
  // serializes styles itself, so `padding: 24` still reaches the inbox as
  // `24px` — pinned above, and the reason an author need not write strings.

  it("escapes text and attribute values", () => {
    const Link = ({ href, label }: { href: string; label: string }) => h("a", { href }, label);
    expect(render(Link, { href: "https://x.test/?a=1&b=2", label: '<b> & "q"' })).toBe(
      '<a href="https://x.test/?a=1&amp;b=2">&lt;b> &amp; &quot;q&quot;</a>'
    );
  });

  // preact 11 dropped `defaultProps` on function components from core: a
  // template that set `Greeting.defaultProps = { name: "friend" }` renders the
  // prop as missing, silently. Default parameters are the supported spelling.
  it("takes a template's defaults from default parameters", () => {
    const Greeting = ({ name = "friend" }: { name?: string }) => h("p", null, `Hello, ${name}!`);
    expect(render<{ name?: string }>(Greeting, {})).toBe("<p>Hello, friend!</p>");
  });

  it("bundles for the Functions isolate and renders there without Node globals", async () => {
    const email = fileURLToPath(new URL("./email.ts", import.meta.url));
    const bundled = await build({
      stdin: {
        contents: `
          import { h } from "preact";
          import { render } from ${JSON.stringify(email)};
          const Card = ({ title }) =>
            h("div", { style: { padding: 24 } }, h("h1", null, title));
          globalThis.__html = render(Card, { title: "Welcome, Ada!" });
        `,
        resolveDir: fileURLToPath(new URL(".", import.meta.url)),
        loader: "js"
      },
      bundle: true,
      format: "esm",
      platform: "neutral",
      write: false,
      logLevel: "silent"
    });
    const code = bundled.outputFiles[0].text;
    // The entry exports nothing, so a complete bundle is a plain script: an
    // `import` left in it would be a dependency esbuild could not inline.
    expect(code).not.toMatch(/^\s*import\s/m);

    const context = vm.createContext({});
    vm.runInContext("delete globalThis.console", context);
    const nodeOnly = vm.runInContext(
      `["setTimeout", "setInterval", "queueMicrotask", "TextEncoder", "TextDecoder",
        "Buffer", "process", "console", "structuredClone"]
        .filter((name) => typeof globalThis[name] !== "undefined")`,
      context
    );
    expect(nodeOnly).toEqual([]);

    vm.runInContext(code, context);
    expect(context.__html).toBe('<div style="padding:24px;"><h1>Welcome, Ada!</h1></div>');
  });
});
