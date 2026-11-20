// Shared by the containers group's jsdom tests (not itself a test file): a `matchMedia` that
// answers the phone query and the reduced-motion query apart (the shared `stubMatchMedia` answers
// every query alike), and the group's stylesheet as text, for the rules a jsdom cannot paint
// (`::backdrop`, transitions).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { vi } from "vitest";
import { PHONE_LAYOUT_QUERY } from "../../shell/phone/usePhoneLayout";

/** Stubs `window.matchMedia`: `phone` answers the phone layout query, `reduce` the reduced-motion
 *  one, everything else no. Undo with `vi.unstubAllGlobals()`. */
export function stubMedia({
  phone = false,
  reduce = false
}: {
  phone?: boolean;
  reduce?: boolean;
}) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches:
      query === PHONE_LAYOUT_QUERY
        ? phone
        : query.includes("prefers-reduced-motion: reduce")
          ? reduce
          : false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {}
  }));
}

const here = dirname(fileURLToPath(import.meta.url));

/** containers.css, comments blanked. */
export const containersCss = readFileSync(join(here, "containers.css"), "utf8").replace(
  /\/\*[\s\S]*?\*\//g,
  ""
);

/** The declarations of every rule whose selector is exactly `selector`, inside `within` (an
 *  at-rule's prelude, matched as a substring) or at the top level when omitted. */
export function declarations(selector: string, within?: string): string {
  const out: string[] = [];
  const stack: string[] = [];
  let start = 0;
  const text = containersCss;
  for (let i = 0; i < text.length; i += 1) {
    const c = text[i];
    if (c === "{") {
      stack.push(text.slice(start, i).trim().replace(/\s+/g, " "));
      start = i + 1;
    } else if (c === "}") {
      const prelude = stack.pop() ?? "";
      const at = stack.filter((s) => s.startsWith("@"));
      const inside = within ? at.some((a) => a.includes(within)) : at.length === 0;
      if (!prelude.startsWith("@") && inside && prelude.split(/\s*,\s*/).includes(selector))
        out.push(text.slice(text.lastIndexOf("{", i) + 1, i));
      start = i + 1;
    } else if (c === ";" && stack.length === 0) {
      start = i + 1;
    }
  }
  return out.join(";").replace(/\s+/g, " ");
}
