const ops = globalThis.Deno.core.ops;
const OxyContext = {
  /**
   * The policy called `name`, read by the host from its policy store. Rejects
   * if it does not exist or the store cannot be read.
   */
  async getPolicy(name) {
    return JSON.parse(await ops.op_oxy_get_policy(name));
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
//# sourceMappingURL=data:application/json;base64,ewogICJ2ZXJzaW9uIjogMywKICAic291cmNlcyI6IFsiLi4vdHMvc2RrLnRzIl0sCiAgIm1hcHBpbmdzIjogIkFBMkJBLE1BQU0sTUFBTyxXQUErRCxLQUFLLEtBQUs7QUFFL0UsTUFBTSxhQUFhO0FBQUE7QUFBQTtBQUFBO0FBQUE7QUFBQSxFQUt4QixNQUFNLFVBQVUsTUFBK0I7QUFDN0MsV0FBTyxLQUFLLE1BQU0sTUFBTSxJQUFJLGtCQUFrQixJQUFJLENBQUM7QUFBQSxFQUNyRDtBQUNGO0FBRU8sTUFBTSxVQUFVO0FBQUE7QUFBQTtBQUFBO0FBQUE7QUFBQSxFQUtyQixZQUFZLEtBQXFCO0FBQy9CLFFBQUksT0FBTztBQUNYLGFBQVMsSUFBSSxHQUFHLElBQUksSUFBSSxRQUFRLEtBQUs7QUFDbkMsY0FBUSxJQUFJLFdBQVcsQ0FBQztBQUN4QixhQUFPLEtBQUssS0FBSyxNQUFNLFFBQVUsTUFBTTtBQUFBLElBQ3pDO0FBQ0EsV0FBTztBQUFBLEVBQ1Q7QUFDRjtBQUVPLE1BQU0sV0FBVztBQUFBO0FBQUEsRUFFdEIsT0FBTyxVQUFnQztBQUNyQyxRQUFJLHNCQUFzQixLQUFLLFVBQVUsUUFBUSxDQUFDO0FBQUEsRUFDcEQ7QUFDRjsiLAogICJuYW1lcyI6IFtdCn0K
