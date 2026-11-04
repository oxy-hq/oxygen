const ops = globalThis.Deno.core.ops;
const OxyContext = {
  /** The policy called `name`, read by the host. Throws if it does not exist. */
  getPolicy(name) {
    return JSON.parse(ops.op_oxy_get_policy(name));
  }
};
const OxyMath = {
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
const OxyAudit = {
  /** Write `decision` to the host's audit log. Throws if the host refuses it. */
  record(decision) {
    ops.op_oxy_write_decision(JSON.stringify(decision));
  }
};
export {
  OxyAudit,
  OxyContext,
  OxyMath
};
//# sourceMappingURL=data:application/json;base64,ewogICJ2ZXJzaW9uIjogMywKICAic291cmNlcyI6IFsiLi4vdHMvc2RrLnRzIl0sCiAgIm1hcHBpbmdzIjogIkFBMkJBLE1BQU0sTUFBTyxXQUErRCxLQUFLLEtBQUs7QUFFL0UsTUFBTSxhQUFhO0FBQUE7QUFBQSxFQUV4QixVQUFVLE1BQXNCO0FBQzlCLFdBQU8sS0FBSyxNQUFNLElBQUksa0JBQWtCLElBQUksQ0FBQztBQUFBLEVBQy9DO0FBQ0Y7QUFFTyxNQUFNLFVBQVU7QUFBQTtBQUFBO0FBQUE7QUFBQTtBQUFBLEVBS3JCLFlBQVksS0FBcUI7QUFDL0IsUUFBSSxPQUFPO0FBQ1gsYUFBUyxJQUFJLEdBQUcsSUFBSSxJQUFJLFFBQVEsS0FBSztBQUNuQyxjQUFRLElBQUksV0FBVyxDQUFDO0FBQ3hCLGFBQU8sS0FBSyxLQUFLLE1BQU0sUUFBVSxNQUFNO0FBQUEsSUFDekM7QUFDQSxXQUFPO0FBQUEsRUFDVDtBQUNGO0FBRU8sTUFBTSxXQUFXO0FBQUE7QUFBQSxFQUV0QixPQUFPLFVBQWdDO0FBQ3JDLFFBQUksc0JBQXNCLEtBQUssVUFBVSxRQUFRLENBQUM7QUFBQSxFQUNwRDtBQUNGOyIsCiAgIm5hbWVzIjogW10KfQo=
