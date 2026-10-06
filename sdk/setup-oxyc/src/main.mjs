/** The action's `main` entry point. The logic, and its tests, are in `setup.mjs`. */

import { realIo } from "./io.mjs";
import { fail, setup } from "./setup.mjs";

const io = realIo();
setup(io).catch((cause) => fail(io, cause));
