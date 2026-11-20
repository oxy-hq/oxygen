// @vitest-environment jsdom
//
// useKeptState across the phone/desk swap, on a stand-in of an app's shape: one
// KeepAcrossLayouts around a phone tree or a desk tree, each mounting its own copy of a screen
// (Bookkeeping's tests, ported). Plus: the provider follows the SDK's own usePhoneLayout when
// `phone` is not given, a value with work under way is pinned, and the reference app's
// `useKeptState(screen, field, initial)` shape still works.

import { act, type Dispatch, type SetStateAction, useEffect, useState } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { KeepAcrossLayouts, useKeptState } from "./keptState";
import { type Mounted, render, stubMatchMedia } from "./phone/phone.harness";
import { usePhoneLayout } from "./phone/usePhoneLayout";

/** The last mount's setter, as work started there would hold it. */
let setters: Dispatch<SetStateAction<string>>[] = [];

function Field({ pinned = false }: { pinned?: boolean }) {
  const [v, setV] = useKeptState<string>("screen:field", "", {
    pinWhile: pinned ? (value: string) => value.startsWith("running") : undefined
  });
  setters.push(setV);
  return <output>{v}</output>;
}
/** A screen whose own read lands after it mounts. */
function LateScreen() {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setReady(true), 10);
    return () => clearTimeout(t);
  }, []);
  return ready ? <Field /> : <p>Loading…</p>;
}
function Layouts({
  phone,
  scope = "a",
  show = true,
  late = false,
  pinned = false
}: {
  phone?: boolean;
  scope?: string;
  show?: boolean;
  late?: boolean;
  pinned?: boolean;
}) {
  const screen = show ? late ? <LateScreen /> : <Field pinned={pinned} /> : null;
  return (
    <KeepAcrossLayouts phone={phone} scope={scope}>
      <Split phone={phone}>{screen}</Split>
    </KeepAcrossLayouts>
  );
}
/** The app's split: a different tree on each side, so the screen remounts. */
function Split({ phone, children }: { phone?: boolean; children: React.ReactNode }) {
  const query = usePhoneLayout();
  return (phone ?? query) ? <section>{children}</section> : <div>{children}</div>;
}

const shown = () => document.querySelector("output")?.textContent ?? null;
const set = (v: string) => act(() => setters[setters.length - 1](v));

let page: Mounted;
beforeEach(() => {
  setters = [];
});
afterEach(async () => {
  await page?.unmount();
  document.body.innerHTML = "";
});

describe("useKeptState", () => {
  it("hands the value to the other layout's mount, both ways", async () => {
    page = await render(<Layouts phone={false} />);
    set("typed");
    await page.rerender(<Layouts phone />);
    expect(document.querySelector("section output")?.textContent).toBe("typed");
    await page.rerender(<Layouts phone={false} />);
    expect(document.querySelector("div > output")?.textContent).toBe("typed");
  });

  it("waits for a twin that mounts only after its own read", async () => {
    page = await render(<Layouts phone={false} />);
    set("typed");
    await page.rerender(<Layouts phone late />);
    expect(shown()).toBeNull();
    await act(() => new Promise((r) => setTimeout(r, 30)));
    expect(shown()).toBe("typed");
  });

  it("drops the value on an ordinary unmount, as useState does", async () => {
    page = await render(<Layouts phone={false} />);
    set("typed");
    await page.rerender(<Layouts phone={false} show={false} />);
    await page.rerender(<Layouts phone={false} />);
    expect(shown()).toBe("");
  });

  it("shows the new mount what work started in the old one wrote after the swap", async () => {
    page = await render(<Layouts phone={false} />);
    const fromOldMount = setters[setters.length - 1];
    await page.rerender(<Layouts phone />);
    act(() => fromOldMount("loaded"));
    expect(shown()).toBe("loaded");
  });

  it("drops a late write from a mount that unmounted for good", async () => {
    page = await render(<Layouts phone={false} />);
    const fromOldMount = setters[setters.length - 1];
    await page.rerender(<Layouts phone={false} show={false} />);
    act(() => fromOldMount("late"));
    await page.rerender(<Layouts phone={false} />);
    expect(shown()).toBe("");
  });

  it("drops what a swap left unclaimed once the screen changes", async () => {
    page = await render(<Layouts phone={false} />);
    set("typed");
    // The phone has no twin of this screen open, then another screen opens.
    await page.rerender(<Layouts phone show={false} />);
    await page.rerender(<Layouts phone show={false} scope='b' />);
    await page.rerender(<Layouts phone scope='b' />);
    expect(shown()).toBe("");
  });

  it("keeps a value with work under way through an ordinary unmount, until the work ends", async () => {
    page = await render(<Layouts phone={false} pinned />);
    set("running 3 of 9");
    await page.rerender(<Layouts phone={false} pinned show={false} />);
    await page.rerender(<Layouts phone={false} pinned scope='b' show={false} />);
    await page.rerender(<Layouts phone={false} pinned scope='b' />);
    expect(shown()).toBe("running 3 of 9");
  });

  it("is plain useState without KeepAcrossLayouts above it", async () => {
    page = await render(<Field />);
    set("typed");
    expect(shown()).toBe("typed");
    await page.rerender(<p />);
    await page.rerender(<Field />);
    expect(shown()).toBe("");
  });
});

/** The app's split with no `phone` given anywhere: the desk mounts the field at once, the phone
 *  only after its own read, so the swap is told from an ordinary unmount by the provider alone. */
function QuerySplit() {
  return usePhoneLayout() ? (
    <section>
      <LateScreen />
    </section>
  ) : (
    <div>
      <Field />
    </div>
  );
}

describe("KeepAcrossLayouts", () => {
  it("follows the SDK's own phone/desk split when `phone` is not given", async () => {
    const media = stubMatchMedia(false);
    const had = window.matchMedia;
    window.matchMedia = media.matchMedia as unknown as typeof window.matchMedia;
    try {
      page = await render(
        <KeepAcrossLayouts scope='a'>
          <QuerySplit />
        </KeepAcrossLayouts>
      );
      expect(document.querySelector("div > output")).not.toBeNull();
      set("typed");
      // A window narrowed past 640px: the phone tree mounts, its field once its read lands, and
      // the value waits for it: the provider read the same query, so it knew this was the swap.
      await media.set(true);
      expect(shown()).toBeNull();
      await act(() => new Promise((r) => setTimeout(r, 30)));
      expect(document.querySelector("section output")?.textContent).toBe("typed");
    } finally {
      window.matchMedia = had;
    }
  });
});

function RefField({ field }: { field: string }) {
  const [v, setV] = useKeptState("/warehouse", field, () => "");
  setters.push(setV);
  return <output data-field={field}>{v}</output>;
}

function RefApp({ phone }: { phone: boolean }) {
  const fields = (
    <>
      <RefField field='container' />
      <RefField field='seal' />
    </>
  );
  return (
    <KeepAcrossLayouts phone={phone} scope='/warehouse'>
      {phone ? <section>{fields}</section> : <div>{fields}</div>}
    </KeepAcrossLayouts>
  );
}

const fieldText = (field: string) =>
  document.querySelector(`output[data-field="${field}"]`)?.textContent;

describe("the reference app's call shape", () => {
  it("keys by screen and field, so two fields of one screen keep their own values", async () => {
    page = await render(<RefApp phone={false} />);
    // The container field's setter: the first of the pair rendered.
    act(() => setters[setters.length - 2]("MSKU0223442"));
    expect(fieldText("container")).toBe("MSKU0223442");
    expect(fieldText("seal")).toBe("");
    await page.rerender(<RefApp phone />);
    expect(fieldText("container")).toBe("MSKU0223442");
    expect(fieldText("seal")).toBe("");
  });
});
