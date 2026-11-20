// Contrast maths, shared by the theme's contrast test (`contrast.test.ts`) and the desk UI's
// (`src/ui/contrast.test.ts`), so both measure one way. Not exported from any entry.
//
// WCAG 2.2 contrast ratio is the gate; APCA Lc is reported and never asserted. Translucent colours
// are composited the way the design validator (validate_tokens.py) does it: source-over in
// gamma-encoded sRGB.

export type Rgb = [number, number, number];
export type Rgba = [number, number, number, number];

const toLinear = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const luminance = ([r, g, b]: Rgb) =>
  0.2126 * toLinear(r) + 0.7152 * toLinear(g) + 0.0722 * toLinear(b);

export function wcagRatio(a: Rgb, b: Rgb): number {
  const [la, lb] = [luminance(a), luminance(b)];
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** APCA-W3 0.0.98G-4g lightness contrast, signed (negative: light text on a dark ground). */
export function apcaLc(text: Rgb, bg: Rgb): number {
  const y = ([r, g, b]: Rgb) => 0.2126729 * r ** 2.4 + 0.7151522 * g ** 2.4 + 0.072175 * b ** 2.4;
  // biome-ignore lint/suspicious/noApproximativeNumericConstant: APCA's black clamp is 1.414 by definition; Math.SQRT2 would change Lc
  const clampBlack = (v: number) => (v > 0.022 ? v : v + (0.022 - v) ** 1.414);
  const ty = clampBlack(y(text));
  const by = clampBlack(y(bg));
  if (Math.abs(by - ty) < 0.0005) return 0;
  if (by > ty) {
    const sapc = (by ** 0.56 - ty ** 0.57) * 1.14;
    return (sapc < 0.1 ? 0 : sapc - 0.027) * 100;
  }
  const sapc = (by ** 0.65 - ty ** 0.62) * 1.14;
  return (sapc > -0.1 ? 0 : sapc + 0.027) * 100;
}

export const composite = ([r, g, b, a]: Rgba, [br, bg, bb]: Rgb): Rgb => [
  a * r + (1 - a) * br,
  a * g + (1 - a) * bg,
  a * b + (1 - a) * bb
];

/** A colour as theme.css writes one: `#rrggbb` or `rgba(r, g, b, a)`. */
export function parseColor(value: string): Rgba | undefined {
  const hex = /^#([0-9a-f]{6})$/.exec(value);
  if (hex) {
    const n = Number.parseInt(hex[1], 16);
    return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255, 1];
  }
  const rgb = /^rgba\((\d+), (\d+), (\d+), ([\d.]+)\)$/.exec(value);
  if (rgb)
    return [Number(rgb[1]) / 255, Number(rgb[2]) / 255, Number(rgb[3]) / 255, Number(rgb[4])];
  return undefined;
}
