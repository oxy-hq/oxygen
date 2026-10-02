// How many test files a tsconfig project actually resolves.
//
// Reads `tsc -p <project> --showConfig` on stdin and prints the number of
// entries in its RESOLVED `files` array whose path contains `.test.`.
//
// The typecheck ratchets in ci.yaml use this as a precondition: their error
// baselines are only meaningful if the program still contains the test files.
// Narrowing `include` — or copying `**/*.test.*` into `exclude`, which the
// sibling tsconfig each `tsconfig.test.json` works around already carries —
// drops the error count far under baseline and reads as a drained backlog.
//
// Three details are load-bearing, each of which was a bug first:
//
//   * count `files`, NOT lines of the document. `--showConfig` echoes the
//     include/exclude globs too, and those contain `.test.` themselves — a
//     line count reads 88 where the program holds 86, and stays non-zero even
//     when `files` is empty, which is the state this exists to catch.
//   * `setEncoding("utf8")` before accumulating. Without it each Buffer chunk
//     decodes separately and a multi-byte character straddling a chunk
//     boundary becomes replacement characters — in the excerpt below, which a
//     human reads.
//   * `process.exitCode`, never `process.exit()`. The latter is documented to
//     exit "even if there are still asynchronous operations pending, including
//     I/O operations to process.stdout and process.stderr", and stderr is a
//     pipe under the Actions runner, hence async — so it can drop the very
//     diagnosis this prints.
//
// tsc writes its DIAGNOSTICS to stdout, so a missing project arrives here as
// `error TS5058: …` rather than JSON. Echoing what was received is what puts
// tsc's own message in the log instead of V8's ten-character snippet.

process.stdin.setEncoding("utf8");

let input = "";
process.stdin
  .on("data", (chunk) => (input += String(chunk)))
  .on("end", () => {
    try {
      const files = JSON.parse(input).files || [];
      console.log(files.filter((f) => f.includes(".test.")).length);
    } catch {
      // Slice by code point, so the excerpt cannot end mid-surrogate-pair.
      const excerpt = Array.from(input).slice(0, 500).join("");
      console.error(`--showConfig did not return JSON. It emitted:\n${excerpt}`);
      process.exitCode = 1;
    }
  });
