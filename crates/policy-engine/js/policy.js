// ts/sdk.ts
var ops = globalThis.Deno.core.ops;
var OxyContext = {
  /**
   * The policy called `name`, read by the host from its policy store. Rejects
   * if it does not exist or the store cannot be read.
   */
  async getPolicy(name) {
    return JSON.parse(await ops.op_oxy_get_policy(name));
  }
};
var OxyMath = {
  /**
   * A stable 32-bit hash of `sku` (FNV-1a over its UTF-16 code units). The same
   * SKU always hashes the same, so it always lands in the same variant.
   */
  hashVariant(sku) {
    let hash = 2166136261;
    for (let i = 0; i < sku.length; i++) {
      hash ^= sku.charCodeAt(i);
      hash = Math.imul(hash, 16777619) >>> 0;
    }
    return hash;
  }
};
var OxyAudit = {
  /** Write `decision` to the host's audit log. Throws if the host refuses it. */
  record(decision) {
    ops.op_oxy_write_decision(JSON.stringify(decision));
  }
};

// ts/policy.ts
async function decide(sku, policyId) {
  const policy = await OxyContext.getPolicy(policyId);
  const totalWeight = policy.variants.reduce((sum, v) => sum + v.weight, 0);
  let bucket = OxyMath.hashVariant(sku) % totalWeight;
  const variant = policy.variants.find((v) => (bucket -= v.weight) < 0);
  if (!variant) throw new Error(`policy ${policy.id} has no variants`);
  const decision = {
    policyId: policy.id,
    sku,
    variant: variant.name,
    priceMultiplier: variant.priceMultiplier
  };
  OxyAudit.record(decision);
  return decision;
}
export {
  decide
};
//# sourceMappingURL=data:application/json;base64,ewogICJ2ZXJzaW9uIjogMywKICAic291cmNlcyI6IFsiLi4vdHMvc2RrLnRzIiwgIi4uL3RzL3BvbGljeS50cyJdLAogICJtYXBwaW5ncyI6ICI7QUEyQkEsSUFBTSxNQUFPLFdBQStELEtBQUssS0FBSztBQUUvRSxJQUFNLGFBQWE7QUFBQTtBQUFBO0FBQUE7QUFBQTtBQUFBLEVBS3hCLE1BQU0sVUFBVSxNQUErQjtBQUM3QyxXQUFPLEtBQUssTUFBTSxNQUFNLElBQUksa0JBQWtCLElBQUksQ0FBQztBQUFBLEVBQ3JEO0FBQ0Y7QUFFTyxJQUFNLFVBQVU7QUFBQTtBQUFBO0FBQUE7QUFBQTtBQUFBLEVBS3JCLFlBQVksS0FBcUI7QUFDL0IsUUFBSSxPQUFPO0FBQ1gsYUFBUyxJQUFJLEdBQUcsSUFBSSxJQUFJLFFBQVEsS0FBSztBQUNuQyxjQUFRLElBQUksV0FBVyxDQUFDO0FBQ3hCLGFBQU8sS0FBSyxLQUFLLE1BQU0sUUFBVSxNQUFNO0FBQUEsSUFDekM7QUFDQSxXQUFPO0FBQUEsRUFDVDtBQUNGO0FBRU8sSUFBTSxXQUFXO0FBQUE7QUFBQSxFQUV0QixPQUFPLFVBQWdDO0FBQ3JDLFFBQUksc0JBQXNCLEtBQUssVUFBVSxRQUFRLENBQUM7QUFBQSxFQUNwRDtBQUNGOzs7QUN0REEsZUFBc0IsT0FBTyxLQUFhLFVBQTJDO0FBQ25GLFFBQU0sU0FBUyxNQUFNLFdBQVcsVUFBVSxRQUFRO0FBQ2xELFFBQU0sY0FBYyxPQUFPLFNBQVMsT0FBTyxDQUFDLEtBQUssTUFBTSxNQUFNLEVBQUUsUUFBUSxDQUFDO0FBR3hFLE1BQUksU0FBUyxRQUFRLFlBQVksR0FBRyxJQUFJO0FBQ3hDLFFBQU0sVUFBVSxPQUFPLFNBQVMsS0FBSyxDQUFDLE9BQU8sVUFBVSxFQUFFLFVBQVUsQ0FBQztBQUNwRSxNQUFJLENBQUMsUUFBUyxPQUFNLElBQUksTUFBTSxVQUFVLE9BQU8sRUFBRSxrQkFBa0I7QUFFbkUsUUFBTSxXQUEyQjtBQUFBLElBQy9CLFVBQVUsT0FBTztBQUFBLElBQ2pCO0FBQUEsSUFDQSxTQUFTLFFBQVE7QUFBQSxJQUNqQixpQkFBaUIsUUFBUTtBQUFBLEVBQzNCO0FBQ0EsV0FBUyxPQUFPLFFBQVE7QUFDeEIsU0FBTztBQUNUOyIsCiAgIm5hbWVzIjogW10KfQo=
