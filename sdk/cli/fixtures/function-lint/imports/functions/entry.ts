// The entry is a router: every problem sits in what it imports. `./steps.js`
// is the TS spelling of ./steps.ts, `./helpers` resolves to helpers/index.ts, the
// package import is not followed, and `../../escape.js` leaves the app.

import { something } from "@oxy-hq/sdk";
import { escaped } from "../../escape.js";
import helper from "./helpers";
import { run } from "./steps.js";

export default async function entry(_req: unknown, ctx: any) {
  return run(ctx, helper(), escaped, something);
}
