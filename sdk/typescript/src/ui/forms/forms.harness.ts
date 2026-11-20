// The forms group's test helper (not a test file): typing into a controlled field the way a
// browser does, so React's change tracking sees it. Only this group's tests use it.

import { act } from "react";

/** Sets a text control's value as typing would, and fires the `input` event React listens to. */
export async function type(el: Element | null, value: string): Promise<void> {
  if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement))
    throw new Error("not a text control");
  const proto =
    el instanceof HTMLInputElement ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  await act(async () => {
    setter?.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

/** Picks an option in a <select>, firing the `change` event React listens to. */
export async function choose(el: Element | null, value: string): Promise<void> {
  if (!(el instanceof HTMLSelectElement)) throw new Error("not a select");
  await act(async () => {
    el.value = value;
    el.dispatchEvent(new Event("change", { bubbles: true }));
  });
}
