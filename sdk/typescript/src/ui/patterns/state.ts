// The patterns' one state rule (CONVENTIONS section 3, controlled first): a value the caller may
// hold (`value` + `onChange`) or leave to the pattern (`initial`), read and written the same way
// either way, so a screen that wants to keep it (`useKeptState`) holds it and one that does not
// passes nothing.

import { useCallback, useState } from "react";

export function useControlled<T>(
  value: T | undefined,
  initial: T,
  onChange: ((next: T) => void) | undefined
): [T, (next: T) => void] {
  const [own, setOwn] = useState<T>(initial);
  const held = value !== undefined;
  const current = held ? value : own;
  const set = useCallback(
    (next: T) => {
      if (!held) setOwn(next);
      onChange?.(next);
    },
    [held, onChange]
  );
  return [current, set];
}

/** A set with one key added or taken away; a new set, so state sees the change. */
export function toggled<K>(set: ReadonlySet<K>, key: K, on: boolean): ReadonlySet<K> {
  const next = new Set(set);
  if (on) next.add(key);
  else next.delete(key);
  return next;
}
