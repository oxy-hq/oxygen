export const getAgentNameFromPath = (path: string) => {
  const parts = path.split("/");
  parts[parts.length - 1] = parts[parts.length - 1].split(".")[0];
  return parts.join(" - ");
};

// eslint-disable-next-line sonarjs/pseudo-random
export const randomKey = () => Math.random().toString(36).substring(2, 15);

export const getShortTitle = (message: string) => {
  const words = message.trim().split(/\s+/);
  const baseTitle = words.slice(0, 8).join(" ");
  let shortTitle = words.length > 8 ? baseTitle : message;

  if (shortTitle.length > 50) {
    shortTitle = `${shortTitle.slice(0, 50)}...`;
  } else if (shortTitle !== message) {
    shortTitle += "...";
  }

  return shortTitle;
};

export const handleDownloadFile = (blob: Blob | MediaSource, fileName: string) => {
  const url = window.URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = fileName;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  window.URL.revokeObjectURL(url);
};

/**
 * Text for a value whose shape is not known: a table cell, a filter value, a
 * field out of an API payload. Strings pass through, numbers and booleans
 * print as themselves, null and undefined are empty, and anything structured
 * is its JSON — never the "[object Object]" that `String()` gives it.
 *
 * It is called while rendering, so it never throws: a bigint inside an object
 * prints as its digits, and a value JSON cannot express (a cycle, an invalid
 * Date) falls back to its type tag.
 */
export const toText = (value: unknown): string => {
  switch (typeof value) {
    case "string":
      return value;
    case "number":
    case "boolean":
    case "bigint":
      return String(value);
    case "object":
      if (value === null) return "";
      try {
        if (value instanceof Date) return value.toISOString();
        return (
          JSON.stringify(value, (_key, v: unknown) => (typeof v === "bigint" ? v.toString() : v)) ??
          ""
        );
      } catch {
        return Object.prototype.toString.call(value);
      }
    default:
      return "";
  }
};
