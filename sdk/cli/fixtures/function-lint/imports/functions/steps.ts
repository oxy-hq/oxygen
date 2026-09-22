import helper from "./helpers/index.js";

export async function run(ctx: any, ...rest: unknown[]) {
  await ctx.email.send({ to: "a@b.c", subject: "s", html: helper() });
  return rest;
}
