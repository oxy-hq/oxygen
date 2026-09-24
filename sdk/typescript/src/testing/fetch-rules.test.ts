// `isSafeOutbound` says what `is_safe_outbound` in `host.rs` says, case by
// case — the examples are the host's own unit tests' shapes.

import { describe, expect, it } from "vitest";
import { isSafeOutbound } from "./fetch-rules";

const safe = (url: string) => isSafeOutbound(new URL(url));

describe("isSafeOutbound", () => {
  it("requires https", () => {
    expect(safe("https://api.example.com/v1")).toBe(true);
    expect(safe("http://api.example.com/v1")).toBe(false);
    expect(safe("ftp://api.example.com/")).toBe(false);
  });

  it("refuses the loopback, private, link-local, broadcast and documentation v4 ranges", () => {
    for (const ip of [
      "127.0.0.1",
      "0.0.0.0",
      "10.1.2.3",
      "172.16.0.1",
      "172.31.255.255",
      "192.168.1.1",
      "169.254.169.254",
      "255.255.255.255",
      "192.0.2.1",
      "198.51.100.7",
      "203.0.113.9"
    ]) {
      expect(safe(`https://${ip}/`), ip).toBe(false);
    }
    expect(safe("https://8.8.8.8/")).toBe(true);
    expect(safe("https://172.32.0.1/")).toBe(true);
  });

  it("folds a v4-mapped v6 address back to v4 before judging it", () => {
    expect(safe("https://[::ffff:169.254.169.254]/")).toBe(false);
    expect(safe("https://[::ffff:10.0.0.5]/")).toBe(false);
    expect(safe("https://[::ffff:8.8.8.8]/")).toBe(true);
  });

  it("judges a NAT64 or v4-compatible address by the embedded v4", () => {
    expect(safe("https://[64:ff9b::169.254.169.254]/")).toBe(false);
    expect(safe("https://[::10.0.0.5]/")).toBe(false);
    expect(safe("https://[64:ff9b::8.8.8.8]/")).toBe(true);
  });

  it("refuses v6 loopback, unspecified, link-local and unique-local", () => {
    expect(safe("https://[::1]/")).toBe(false);
    expect(safe("https://[::]/")).toBe(false);
    expect(safe("https://[fe80::1]/")).toBe(false);
    expect(safe("https://[fd12:3456::1]/")).toBe(false);
    expect(safe("https://[2606:4700::1111]/")).toBe(true);
  });

  it("refuses the literal host names and the internal suffixes, case-insensitively", () => {
    for (const host of ["localhost", "LOCALHOST", "ip6-localhost", "ip6-loopback"]) {
      expect(safe(`https://${host}/`), host).toBe(false);
    }
    for (const host of [
      "db.internal",
      "printer.local",
      "box.localdomain",
      "api.svc",
      "api.default.svc.cluster.local",
      "Api.Internal"
    ]) {
      expect(safe(`https://${host}/`), host).toBe(false);
    }
    expect(safe("https://internal.example.com/")).toBe(true);
  });
});
