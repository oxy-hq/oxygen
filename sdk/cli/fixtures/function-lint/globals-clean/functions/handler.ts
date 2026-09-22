// Every absent global named, none of them the missing value: Buffer.from("x")
// in a line comment, /* new TextDecoder() */ in a block comment, and below a
// shadowing const, a shadowing import, a shadowing function, strings, a
// template literal, a regex, a `typeof` guard, a type annotation, a property
// of another object, and `ctx.crypto.subtle` — which is ctx's, not the global's.

import type { Blob } from "./polyfill.js";
import { TextEncoder } from "./polyfill.js";

const Buffer = { from: (text: string) => text };

function process(text: string): string {
  return text;
}

export default async function handler(_req: unknown, ctx: any) {
  const a = Buffer.from("x");
  const enc = new TextEncoder();
  const s = "Buffer.from and new TextEncoder() in a string";
  const t = `process.env.${a} in a template`;
  const re = /new FormData\(/;
  const guarded = typeof FormData !== "undefined";
  const prop = ctx.crypto.subtle;
  const node = { Buffer: 1 }.Buffer;
  let body: Blob | undefined;
  const p = process("x");
  return Response.json({ a, enc, s, t, re, guarded, prop, node, body, p });
}
