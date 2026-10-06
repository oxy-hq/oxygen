/** The action's `post` entry point. The logic, and its tests, are in `cleanup.mjs`. */

import { cleanup } from "./cleanup.mjs";
import { realIo, warning } from "./io.mjs";

const io = realIo();
// `cleanup` does not throw by design. If it ever does, that is still not a
// reason to fail a job whose work is done.
cleanup(io).catch((cause) => {
  warning(io, `token cleanup did not finish: ${cause instanceof Error ? cause.message : cause}`);
});
