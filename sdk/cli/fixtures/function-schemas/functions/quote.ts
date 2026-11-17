// `Input` and `Output` declared beside the handler: the common case.
import type { OxyFunctionContext, OxyFunctionRequest } from "@oxy-hq/sdk";

/** Ask for a delivery quote. */
export interface Input {
  /**
   * The store to quote from.
   * @asType integer
   * @minimum 1
   */
  store: number;
  kind: "pickup" | "delivery";
  /** When the customer wants it, if not now. */
  at?: Date;
}

export type Output = { price: number; currency: "USD" | "EUR" };

export default async function quote(
  req: OxyFunctionRequest,
  _ctx: OxyFunctionContext
): Promise<Output> {
  const input = JSON.parse(req.body) as Input;
  return { price: input.store * 2, currency: "USD" };
}
