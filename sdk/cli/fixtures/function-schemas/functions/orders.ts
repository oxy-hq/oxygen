// Output from the handler's return type, through a relative type import.
import type { OxyFunctionContext, OxyFunctionRequest } from "@oxy-hq/sdk";
import type { Order } from "./types";

export default async function orders(
  _req: OxyFunctionRequest,
  ctx: OxyFunctionContext
): Promise<{ orders: Order[]; total: number }> {
  const rows = (await ctx.warehouse.query("SELECT 1")) as Order[];
  return { orders: rows, total: rows.length };
}
