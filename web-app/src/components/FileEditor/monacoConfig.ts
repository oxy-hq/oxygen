// These two imports ARE the resolution guard — see the block below.
import type * as MonacoApi from "monaco-editor/esm/vs/editor/editor.api";
import type * as MonacoApiJs from "monaco-editor/esm/vs/editor/editor.api.js";
import { configureMonacoYaml } from "monaco-yaml";
import { monacoGitHubDarkDefaultTheme } from "@/components/FileEditor/hooks/github-dark-theme";
import YamlWorker from "@/components/FileEditor/hooks/yaml.worker.js?worker";

// Compile-time guards for the two `monaco-editor/esm/vs/editor/editor.api`
// mappings in tsconfig.app.json. monaco 0.56 stopped exporting that path, but
// `@monaco-editor/react` (for `Monaco`) and `monaco-types` (for
// `configureMonacoYaml`'s parameter, via `monaco-yaml`) still import it from
// inside their own `.d.ts` files, where `skipLibCheck: true` swallows the
// failed resolution. Without something here, deleting a mapping as an unneeded
// workaround — or monaco moving the file again — leaves every check green while
// those two APIs quietly stop being typed.
//
// Guarding the CONSUMER types only half works, and which half is not a matter
// of taste. Measured against this tsc by deleting each mapping in turn:
//
//   `configureMonacoYaml`'s parameter degrades to an empty namespace (its
//   source, `monaco-types`, does `export *` from the vanished module), and a
//   structural `extends { editor: unknown }` catches that.
//
//   `Monaco` degrades to TypeScript's *error* type, which is assignable in
//   both directions and answers no type-level predicate the way `any` does:
//   `0 extends 1 & T`, `unknown extends T`, and a tuple-wrapped structural
//   check were all tried against the real degraded type and all three PASS.
//   None of the three catches it against this tsc; treat that as measured
//   rather than as a proof that no predicate could, and re-test after a
//   TypeScript upgrade before relying on it.
//
// So resolution is guarded where it can actually fail loudly: by importing both
// mapped specifiers here. This is a `.ts` file, not a dependency's `.d.ts`, so
// `skipLibCheck` does not apply and an unresolvable module is a hard TS2307.
export const monacoApiMappingsResolve: [typeof MonacoApi, typeof MonacoApiJs] extends [
  { editor: unknown },
  { editor: unknown }
]
  ? true
  : never = true;

// ...and the consumer type the app actually uses is guarded for BOTH modes.
// The mapping half is covered above, so this one's remaining job is a change
// in what monaco-yaml/monaco-types imports — a spelling neither entry covers.
// That can degrade either way, so it needs both arms: `0 extends 1 & T` for
// `any` (a structural check can't see it — a conditional with an `any` check
// type yields the union of both branches, so `true | never` = `true`), and the
// structural check for the empty namespace (which `0 extends 1 & T` misses,
// since `{} extends 1` is false). Exported because `noUnusedLocals` is on;
// tree-shaken out of the bundle.
export const monacoYamlTypesResolved: 0 extends 1 & Parameters<typeof configureMonacoYaml>[0]
  ? never
  : Parameters<typeof configureMonacoYaml>[0] extends { editor: unknown }
    ? true
    : never = true;

type WindowWithMonaco = Window & {
  MonacoEnvironment?: {
    getWorker?: (workerId?: string, label?: string) => Worker | Promise<Worker>;
  };
};

export const configureMonacoEnvironment = () => {
  (window as WindowWithMonaco).MonacoEnvironment = {
    getWorker: (_workerId?: string, label?: string): Worker | Promise<Worker> => {
      switch (label) {
        case "yaml":
          return new YamlWorker();
        default:
          // monaco-editor 0.56 rewrote its `exports` map to
          //     "./*.js": "./esm/vs/*.js",
          //     "./*":    "./esm/vs/*.js"
          // so the old `monaco-editor/esm/vs/...` spelling double-prefixes and
          // no longer resolves. This is the 0.56+ spelling of the same file
          // (it matches the "./*.js" entry).
          return new Worker(new URL("monaco-editor/editor/editor.worker.js", import.meta.url), {
            type: "module"
          });
      }
    }
  };
};

/** The monaco namespace, sourced from the `paths` mappings this file pins.
 *
 * Prefer this over `@monaco-editor/react`'s `Monaco` re-export anywhere the
 * type crosses a module boundary. Structurally identical today — both are
 * `typeof import("monaco-editor/esm/vs/editor/editor.api")` — but this one
 * resolves through a mapping guarded by TS2307 above, so it cannot quietly
 * become the error type if that package changes what it imports.
 */
export type MonacoNamespace = typeof MonacoApi;

export const configureMonaco = (monaco: MonacoNamespace) => {
  monaco.editor.defineTheme("github-dark", monacoGitHubDarkDefaultTheme);

  configureMonacoYaml(monaco, {
    enableSchemaRequest: true,
    hover: false,
    completion: true,
    validate: true,
    format: { enable: true },
    schemas: [
      {
        // Canonical automation files (formerly automations / automations).
        fileMatch: [
          "**/*.automation.yml",
          "**/*.automation.yaml",
          "**/*.procedure.yml",
          "**/*.procedure.yaml"
        ],
        // `workflow.json`, NOT `automation.json`. Both were committed, both
        // titled `Automation`, and only this one is generated from the Rust
        // `Automation` type by `oxy gen-config-schema` — the other was a stale
        // copy nothing regenerated, 6 KB behind, and it is what this editor
        // validated against. The file kind is `.automation.yml`; the schema
        // file keeps the older name because that is what the generator writes.
        uri: "https://raw.githubusercontent.com/oxy-hq/oxygen/refs/heads/main/json-schemas/workflow.json"
      },
      {
        fileMatch: ["**/config.yml", "**/config.yaml"],
        uri: "https://raw.githubusercontent.com/oxy-hq/oxygen/refs/heads/main/json-schemas/config.json"
      }
    ]
  });
};
