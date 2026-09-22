# Oxy Functions lint fixtures

Small custom apps for `src/publish/function-lint.test.ts` and the end-to-end
cases in `commands/validate.test.ts` / `commands/publish.test.ts`. Each rule
has a violating app and a clean twin; `imports/` proves relative imports are
followed and `escape.ts` (outside every app directory) that the walk stops at
the app.

Outside `src/` on purpose: these sources call `ctx` untyped and use globals the
isolate lacks, so `tsc` and `biome check src` must never see them.
