// `is_safe_outbound` and `is_public_ip` from `host.rs`, in TypeScript: the
// first-layer check every `ctx.fetch` URL meets before anything is sent. The
// names and suffixes come from `FETCH_RULES` in the contract, which the Rust
// drift test holds to the host; the IP arithmetic below follows the Rust
// line by line and is pinned by `fetch-rules.test.ts`.
//
// What this cannot do, because no unit test can: the host's second layer
// resolves the name and refuses a public name that points at a private
// address (`PublicOnlyDnsResolver`). A test that passes here with a hostname
// says nothing about where that name resolves.

import { FETCH_RULES } from "./host-contract";

/** The host's verdict on a URL: allowed, or refused before sending. */
export function isSafeOutbound(url: URL): boolean {
  if (url.protocol !== `${FETCH_RULES.scheme}:`) return false;
  const host = url.hostname;
  if (host === "") return false;
  // `URL` keeps the brackets on a v6 literal; the Rust strips them the same way.
  const forIp = host.startsWith("[") && host.endsWith("]") ? host.slice(1, -1) : host;
  const ip = parseIp(forIp);
  if (ip) return isPublicIp(ip);
  const lower = host.toLowerCase();
  if ((FETCH_RULES.literalHosts as readonly string[]).includes(lower)) return false;
  return !FETCH_RULES.internalSuffixes.some((suffix) => lower.endsWith(suffix));
}

type Ip = { v4: [number, number, number, number] } | { v6: number[] };

/** A literal IPv4 or IPv6 address, or `null` when `s` is a name. */
function parseIp(s: string): Ip | null {
  const v4 = parseV4(s);
  if (v4) return { v4 };
  const v6 = parseV6(s);
  return v6 ? { v6 } : null;
}

function parseV4(s: string): [number, number, number, number] | null {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(s);
  if (!m) return null;
  const parts = m.slice(1).map(Number);
  if (parts.some((p) => p > 255)) return null;
  return parts as [number, number, number, number];
}

/** Eight 16-bit segments, or `null`. Accepts `::`, and a dotted v4 tail. */
function parseV6(s: string): number[] | null {
  if (!s.includes(":")) return null;
  const zone = s.indexOf("%");
  const text = zone >= 0 ? s.slice(0, zone) : s;
  const halves = text.split("::");
  if (halves.length > 2) return null;
  const head = halves[0] === "" ? [] : halves[0].split(":");
  const tail = halves.length === 2 && halves[1] !== "" ? halves[1].split(":") : [];
  const words = (groups: string[]): number[] | null => {
    const out: number[] = [];
    for (const [i, g] of groups.entries()) {
      if (g.includes(".")) {
        if (i !== groups.length - 1) return null;
        const v4 = parseV4(g);
        if (!v4) return null;
        out.push((v4[0] << 8) | v4[1], (v4[2] << 8) | v4[3]);
      } else {
        if (!/^[0-9a-fA-F]{1,4}$/.test(g)) return null;
        out.push(Number.parseInt(g, 16));
      }
    }
    return out;
  };
  const h = words(head);
  const t = words(tail);
  if (!h || !t) return null;
  if (halves.length === 1) return h.length === 8 ? h : null;
  const fill = 8 - h.length - t.length;
  if (fill < 1) return null;
  return [...h, ...new Array<number>(fill).fill(0), ...t];
}

/** `is_public_ip` in `host.rs`. */
export function isPublicIp(ip: Ip): boolean {
  const canonical = "v6" in ip ? (mappedV4(ip.v6) ?? ip) : ip;
  if ("v4" in canonical) return isPublicV4(canonical.v4);
  const seg = canonical.v6;
  if (seg.every((w) => w === 0)) return false; // unspecified
  if (seg.slice(0, 7).every((w) => w === 0) && seg[7] === 1) return false; // loopback
  // NAT64 (`64:ff9b::/96`) and IPv4-compatible (`::/96`) embed a v4 address
  // in the low 32 bits: judge the embedded address.
  const embedded = embeddedV4(seg);
  if (embedded) return isPublicV4(embedded);
  const first = seg[0];
  const linkLocal = (first & 0xffc0) === 0xfe80;
  const uniqueLocal = (first & 0xfe00) === 0xfc00;
  return !linkLocal && !uniqueLocal;
}

function isPublicV4([a, b, c, d]: [number, number, number, number]): boolean {
  if (a === 127) return false; // loopback
  if (a === 0 && b === 0 && c === 0 && d === 0) return false; // unspecified
  const isPrivate = a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168);
  const linkLocal = a === 169 && b === 254;
  const broadcast = a === 255 && b === 255 && c === 255 && d === 255;
  const documentation =
    (a === 192 && b === 0 && c === 2) ||
    (a === 198 && b === 51 && c === 100) ||
    (a === 203 && b === 0 && c === 113);
  return !isPrivate && !linkLocal && !broadcast && !documentation;
}

/** `to_canonical()`: an IPv4-mapped address (`::ffff:a.b.c.d`) as its v4. */
function mappedV4(seg: number[]): Ip | null {
  if (seg.slice(0, 5).every((w) => w === 0) && seg[5] === 0xffff) {
    return { v4: [seg[6] >> 8, seg[6] & 0xff, seg[7] >> 8, seg[7] & 0xff] };
  }
  return null;
}

/** `embedded_ipv4`: the v4 in a `::/96` or `64:ff9b::/96` address. */
function embeddedV4(seg: number[]): [number, number, number, number] | null {
  const compatible = seg.slice(0, 6).every((w) => w === 0);
  const nat64 = seg[0] === 0x64 && seg[1] === 0xff9b && seg.slice(2, 6).every((w) => w === 0);
  if (!compatible && !nat64) return null;
  return [seg[6] >> 8, seg[6] & 0xff, seg[7] >> 8, seg[7] & 0xff];
}
