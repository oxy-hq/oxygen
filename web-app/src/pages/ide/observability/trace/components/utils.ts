// Recursively parse nested JSON strings
export function deepParseJson(obj: unknown): unknown {
  if (typeof obj === "string") {
    try {
      const parsed = JSON.parse(obj);
      return deepParseJson(parsed);
    } catch {
      return obj;
    }
  }
  if (Array.isArray(obj)) {
    return obj.map(deepParseJson);
  }
  if (obj !== null && typeof obj === "object") {
    const result: Record<string, unknown> = {};
    for (const [key, val] of Object.entries(obj)) {
      result[key] = deepParseJson(val);
    }
    return result;
  }
  return obj;
}
