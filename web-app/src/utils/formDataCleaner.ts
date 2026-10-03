/**
 * Utility functions for cleaning form data by removing empty/meaningless values
 */

const isMeaningfulValue = (value: unknown): boolean => {
  if (value === undefined || value === null || value === "") {
    return false;
  }
  if (typeof value === "number" && Number.isNaN(value)) {
    return false;
  }
  if (typeof value === "string" && !value.trim()) {
    return false;
  }
  return true;
};

/**
 * What to do with one key's value that cleaning would otherwise strip or
 * rewrite: `"verbatim"` keeps it exactly as written, contents included;
 * `"keep"` still cleans inside it but keeps the key when the result is empty.
 */
type Preserve = (key: string, value: unknown) => "verbatim" | "keep" | undefined;

export interface CleanOptions {
  /**
   * For keys where an empty value and a missing key mean different things to
   * whatever reads the result — a required list, an explicit `null` — or where
   * the value is the user's own data rather than a form field. Applies at
   * every depth.
   */
  preserve?: Preserve;
}

export const cleanObject = (
  obj: Record<string, unknown>,
  options: CleanOptions = {}
): Record<string, unknown> | null => {
  const cleaned: Record<string, unknown> = {};

  Object.entries(obj).forEach(([key, value]) => {
    if (value === undefined) return;
    const mode = options.preserve?.(key, value);
    if (mode === "verbatim") {
      cleaned[key] = value;
    }
    // Handle arrays
    else if (Array.isArray(value)) {
      const cleanedArray = value
        .map((item) => {
          if (item && typeof item === "object") {
            return cleanObject(item as Record<string, unknown>, options);
          }
          return isMeaningfulValue(item) ? item : null;
        })
        .filter((item) => item !== null);

      if (cleanedArray.length > 0 || mode === "keep") {
        cleaned[key] = cleanedArray;
      }
    }
    // Handle nested objects recursively
    else if (value && typeof value === "object") {
      const cleanedNested = cleanObject(value as Record<string, unknown>, options);
      if (cleanedNested) {
        cleaned[key] = cleanedNested;
      } else if (mode === "keep") {
        cleaned[key] = {};
      }
    }
    // Handle primitive values
    else if (isMeaningfulValue(value) || mode === "keep") {
      cleaned[key] = value;
    }
  });

  return Object.keys(cleaned).length > 0 ? cleaned : null;
};
