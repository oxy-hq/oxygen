// The dummy Oxy SDK a policy script imports. Each call that needs the host
// crosses into Rust through one named op; values cross as JSON text.

export interface Variant {
  name: string;
  weight: number;
  priceMultiplier: number;
}

export interface Policy {
  id: string;
  variants: Variant[];
  bounds: { minMultiplier: number; maxMultiplier: number };
}

export interface PolicyDecision {
  policyId: string;
  sku: string;
  variant: string;
  priceMultiplier: number;
}

interface HostOps {
  op_oxy_get_policy(name: string): string;
  op_oxy_write_decision(decision: string): void;
}

const ops = (globalThis as unknown as { Deno: { core: { ops: HostOps } } }).Deno.core.ops;

export const OxyContext = {
  /** The policy called `name`, read by the host. Throws if it does not exist. */
  getPolicy(name: string): Policy {
    return JSON.parse(ops.op_oxy_get_policy(name)) as Policy;
  },
};

export const OxyMath = {
  /**
   * A stable 32-bit hash of `sku` (FNV-1a over its UTF-16 code units). The same
   * SKU always hashes the same, so it always lands in the same variant.
   */
  hashVariant(sku: string): number {
    let hash = 0x811c9dc5;
    for (let i = 0; i < sku.length; i++) {
      hash ^= sku.charCodeAt(i);
      hash = Math.imul(hash, 0x01000193) >>> 0;
    }
    return hash;
  },
};

export const OxyAudit = {
  /** Write `decision` to the host's audit log. Throws if the host refuses it. */
  record(decision: PolicyDecision): void {
    ops.op_oxy_write_decision(JSON.stringify(decision));
  },
};
