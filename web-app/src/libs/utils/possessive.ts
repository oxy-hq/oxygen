/** "Northwind's", and "Northwind Traders'" for a name that already ends in s. */
export function possessive(name: string): string {
  const trimmed = name.trim();
  return /s$/i.test(trimmed) ? `${trimmed}'` : `${trimmed}'s`;
}
