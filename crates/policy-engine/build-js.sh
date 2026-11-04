#!/usr/bin/env bash
# Compile ts/ into the js/ the engine loads. Rerun after editing anything in ts/,
# and commit js/ with the change: the engine runs JavaScript only.
#
# Same esbuild version as the Functions template and the same flags as
# `oxyc publish` (sdk/cli/src/publish/functions.ts), so a script that runs here
# was built the way a published Function is.
set -euo pipefail
cd "$(dirname "$0")"
esbuild=(pnpm dlx esbuild@0.28.2 --format=esm --platform=neutral --sourcemap=inline --sources-content=false --log-level=warning)
# The policy, bundled with the SDK it imports: one self-contained module.
"${esbuild[@]}" ts/policy.ts --bundle --outfile=js/policy.js
# The SDK alone, for test scripts that import it.
"${esbuild[@]}" ts/sdk.ts --outfile=js/sdk.js
