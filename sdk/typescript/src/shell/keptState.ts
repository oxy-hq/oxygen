// Started from pokehouse-oxy:apps/bookkeeping/src/lib/keptState.tsx (BEYOND-GALLERY row 6). Of
// the five copies, all different, it is the one that behaves exactly as `useState` everywhere
// except across the swap: it tells an unmount the swap caused from an ordinary one, so leaving a
// screen or closing a pane still drops the value; it waits for a twin that mounts only after its
// own read lands; work still in flight in the old mount keeps writing to the value the new mount
// shows, while a late write from a mount gone for good is dropped, as React drops it; and
// `pinWhile` keeps a value with work under way (a bulk post) through any unmount. The others drop
// on a route change only (the reference's `forgetOtherScreens`, Marketing's `forgetKept`), so a
// pane closed and reopened on the same screen comes back with stale input; Accounts Insight's
// epoch (a save landing after `forget` is dropped) is covered here by the holder's `released`
// mark. Kept as it is, written with `createElement` so the file stays `.ts`. New: the
// provider reads the SDK's own `usePhoneLayout` when `phone` is not given (the SDK makes the swap,
// so it owns what survives it), and the reference's call shape
// `useKeptState(screen, field, initial)` still works, so moving it is an import swap plus the
// provider.
//
// State that survives the swap between an app's phone and desk layouts. The app renders one tree
// on a phone and another on the desk (`usePhoneLayout()`), so crossing the breakpoint (a resize,
// split screen, a tablet turned) unmounts every screen below the split and mounts its twin.
// What a screen held in `useState` went with it: half a form, files dropped but not yet loaded,
// a row picked. `useKeptState` is `useState` whose value lives in a store above the split:
//
//   <KeepAcrossLayouts scope={screenPath}>
//     {phone ? <OrdersPhone /> : <OrdersDesk />}
//   </KeepAcrossLayouts>
//
//   const [draft, setDraft] = useKeptState("orders:draft", "");
//
// - An ordinary unmount drops the value, exactly as `useState` would.
// - An unmount caused by the swap leaves the value for the other layout's mount of the same key,
//   whether it comes in the same commit or once its own read has landed.
// - Values a swap left and no mount claimed are dropped when `scope` (the screen) changes.
// - Without `KeepAcrossLayouts` above it (a component on its own, a test) it is plain useState.
//
// A key is one mounted thing: two components mounted at once must not share one.

import {
  createContext,
  createElement,
  type Dispatch,
  type ReactNode,
  type SetStateAction,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore
} from "react";
import { usePhoneLayout } from "./phone/usePhoneLayout";

/** One mount of a key. `line` is shared by the mounts a swap hands it between. */
interface Holder {
  line: object;
  released: boolean;
}

interface Entry {
  value: unknown;
  /** The mount holding it now; null while it waits for the other layout's. */
  holder: Holder | null;
  line: object;
  /** Work is under way in it: it outlives every unmount and every change of screen. */
  pinned: boolean;
}

type PinWhile = ((value: unknown) => boolean) | undefined;

class KeptStore {
  /** The layout last committed. An unmount compares the one it mounted in. */
  phone: boolean;
  private entries = new Map<string, Entry>();
  private listeners = new Map<string, Set<() => void>>();

  constructor(phone: boolean) {
    this.phone = phone;
  }

  peek(key: string): Entry | undefined {
    return this.entries.get(key);
  }

  subscribe(key: string, fn: () => void): () => void {
    let set = this.listeners.get(key);
    if (!set) {
      set = new Set();
      this.listeners.set(key, set);
    }
    set.add(fn);
    return () => set.delete(fn);
  }

  private notify(key: string) {
    for (const fn of this.listeners.get(key) ?? []) fn();
  }

  write(
    key: string,
    holder: Holder,
    next: SetStateAction<unknown>,
    current: unknown,
    pinWhile: PinWhile
  ) {
    let e = this.entries.get(key);
    if (!e) {
      // Gone with an ordinary unmount: a late write from it is dropped, as React drops a set on
      // an unmounted component.
      if (holder.released) return;
      e = { value: current, holder, line: holder.line, pinned: false };
      this.entries.set(key, e);
    } else if (e.line !== holder.line) {
      return;
    }
    const value = typeof next === "function" ? (next as (prev: unknown) => unknown)(e.value) : next;
    e.pinned = pinWhile?.(value) ?? false;
    if (Object.is(value, e.value)) return;
    e.value = value;
    this.notify(key);
  }

  claim(key: string, holder: Holder, value: unknown, pinWhile: PinWhile) {
    holder.released = false;
    const e = this.entries.get(key);
    if (e && e.line === holder.line) e.holder = holder;
    else
      this.entries.set(key, {
        value,
        holder,
        line: holder.line,
        pinned: pinWhile?.(value) ?? false
      });
  }

  release(key: string, holder: Holder, swapped: boolean) {
    holder.released = true;
    const e = this.entries.get(key);
    if (!e || e.holder !== holder) return;
    if (swapped || e.pinned) e.holder = null;
    else this.entries.delete(key);
  }

  /** Values a swap left that no mount took, unless work is under way in them. */
  dropUnclaimed() {
    for (const [key, e] of this.entries)
      if (e.holder === null && !e.pinned) this.entries.delete(key);
  }
}

const KeptContext = createContext<{ store: KeptStore; phone: boolean } | null>(null);

export interface KeepAcrossLayoutsProps {
  /** The layout being rendered; defaults to `usePhoneLayout()`, the SDK's own split. */
  phone?: boolean;
  /** The screen (its path): a change drops what a swap left unclaimed. */
  scope: string;
  children: ReactNode;
}

/** Around both of the app's layouts, above the phone/desk split. */
export function KeepAcrossLayouts({ phone, scope, children }: KeepAcrossLayoutsProps) {
  const query = usePhoneLayout();
  const layout = phone ?? query;
  const [store] = useState(() => new KeptStore(layout));
  // A layout effect: it runs before the old tree's passive cleanups, which read it to tell a
  // swap from an ordinary unmount.
  useLayoutEffect(() => {
    store.phone = layout;
  }, [store, layout]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs when the screen changes.
  useEffect(() => store.dropUnclaimed(), [store, scope]);
  const value = useMemo(() => ({ store, phone: layout }), [store, layout]);
  return createElement(KeptContext.Provider, { value }, children);
}

export interface KeptStateOptions<T> {
  /** True while work is under way in the value (a run, a write in flight): until it turns
   *  false, no unmount and no change of screen drops it, so the next mount of the key shows the
   *  work and the work's own writes still land. */
  pinWhile?: (value: T) => boolean;
}

const NONE = Symbol("none");
const noSubscription = () => () => {};

/**
 * `useState` that a swap between the app's phone and desk layouts hands to the other layout's
 * mount of the same `key`. Without `KeepAcrossLayouts` above it, it is plain `useState`.
 */
export function useKeptState<T>(
  key: string,
  initial: T | (() => T),
  options?: KeptStateOptions<T>
): [T, Dispatch<SetStateAction<T>>];
/** The reference app's shape: the screen it belongs to, then the field, then the initial value. */
export function useKeptState<T>(
  screen: string,
  field: string,
  initial: () => T
): [T, Dispatch<SetStateAction<T>>];
export function useKeptState<T>(
  first: string,
  second: unknown,
  third?: unknown
): [T, Dispatch<SetStateAction<T>>] {
  const reference = typeof third === "function";
  const key = reference ? `${first}#${second as string}` : first;
  const initial = (reference ? third : second) as T | (() => T);
  const options = (reference ? undefined : third) as KeptStateOptions<T> | undefined;
  return useKept(key, initial, options);
}

function useKept<T>(
  key: string,
  initial: T | (() => T),
  options: KeptStateOptions<T> = {}
): [T, Dispatch<SetStateAction<T>>] {
  const ctx = useContext(KeptContext);
  const store = ctx?.store ?? null;
  // One holder per key: a key that changes joins that key's value, as a fresh mount would.
  const holder = useMemo<Holder>(
    () => ({ line: store?.peek(key)?.line ?? {}, released: false }),
    [store, key]
  );
  const pinRef = useRef<PinWhile>(undefined);
  pinRef.current = options.pinWhile as PinWhile;
  const [local, setLocal] = useState<T>(() => {
    const e = store?.peek(key);
    if (e && e.line === holder.line) return e.value as T;
    return typeof initial === "function" ? (initial as () => T)() : initial;
  });

  const subscribe = useCallback(
    (fn: () => void) => (store ? store.subscribe(key, fn) : noSubscription()),
    [store, key]
  );
  const snapshot = useCallback(() => {
    const e = store?.peek(key);
    return e && e.line === holder.line ? e.value : NONE;
  }, [store, key, holder]);
  const kept = useSyncExternalStore(subscribe, snapshot, snapshot);
  const value = (kept === NONE ? local : kept) as T;
  const last = useRef(value);
  last.current = value;

  // Claimed in a layout effect, so a twin mounting in the same commit as the swap holds the
  // value before the old mount's cleanup looks at it.
  useLayoutEffect(() => {
    if (!store) return;
    store.claim(key, holder, last.current, pinRef.current);
  }, [store, key, holder]);
  // Released in a passive cleanup, which runs after KeepAcrossLayouts has recorded the new
  // layout: a different one means this unmount is the swap.
  const phoneAtMount = ctx?.phone ?? false;
  useEffect(() => {
    if (!store) return;
    store.claim(key, holder, last.current, pinRef.current);
    return () => store.release(key, holder, store.phone !== phoneAtMount);
  }, [store, key, holder, phoneAtMount]);

  const set = useCallback<Dispatch<SetStateAction<T>>>(
    (next) => {
      if (!store) return setLocal(next);
      store.write(key, holder, next as SetStateAction<unknown>, last.current, pinRef.current);
    },
    [store, key, holder]
  );
  return [value, set];
}
