// A policy script: route a SKU to a price variant by weight, audit the
// decision, return it.

import { OxyAudit, OxyContext, OxyMath, type PolicyDecision } from "./sdk.ts";

export function decide(sku: string): PolicyDecision {
  const policy = OxyContext.getPolicy("mock");
  const totalWeight = policy.variants.reduce((sum, v) => sum + v.weight, 0);

  // Walk the variants until the SKU's bucket falls inside one's weight band.
  let bucket = OxyMath.hashVariant(sku) % totalWeight;
  const variant = policy.variants.find((v) => (bucket -= v.weight) < 0);
  if (!variant) throw new Error(`policy ${policy.id} has no variants`);

  const decision: PolicyDecision = {
    policyId: policy.id,
    sku,
    variant: variant.name,
    priceMultiplier: variant.priceMultiplier,
  };
  OxyAudit.record(decision);
  return decision;
}
