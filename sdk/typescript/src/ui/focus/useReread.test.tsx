// @vitest-environment jsdom
//
// Try again mounts the reading part again, and only that part: the reference's `useReread` keys
// what it is given; a `RereadBoundary` remounts its children and nothing outside it, a nested one
// re-reads only itself, and `useRereadAction` reaches the nearest one (none outside a boundary).

import { useEffect, useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { $, click, render } from "../ui.harness";
import { RereadBoundary, useReread, useRereadAction } from "./useReread";

afterEach(() => {
  document.body.innerHTML = "";
});

/** Counts its own mounts, keeps a local draft, and offers the nearest re-read. */
function Reader({ id, mounts }: { id: string; mounts: Record<string, number> }) {
  const reread = useRereadAction();
  const [draft, setDraft] = useState("");
  useEffect(() => {
    mounts[id] = (mounts[id] ?? 0) + 1;
  }, [id, mounts]);
  return (
    <div>
      <button type='button' id={`${id}-type`} onClick={() => setDraft("typed")}>
        type
      </button>
      <span id={`${id}-draft`}>{draft}</span>
      <button type='button' id={`${id}-reread`} onClick={reread} disabled={!reread}>
        reread
      </button>
    </div>
  );
}

/** The reference's form: the part keyed on the attempt mounts again on Try again. */
function Screen({ mounts }: { mounts: Record<string, number> }) {
  const [attempt, reread] = useReread();
  return (
    <>
      <span id='attempt'>{attempt}</span>
      <button type='button' id='again' onClick={reread}>
        Try again
      </button>
      <Reader key={attempt} id='part' mounts={mounts} />
    </>
  );
}

describe("useReread", () => {
  it("counts attempts; the part keyed on it mounts again", async () => {
    const mounts: Record<string, number> = {};
    await render(<Screen mounts={mounts} />);
    expect($("#attempt")?.textContent).toBe("0");
    await click($("#again"));
    expect($("#attempt")?.textContent).toBe("1");
    expect(mounts.part).toBe(2);
  });
});

describe("RereadBoundary", () => {
  it("re-reads only what is inside it: the part outside keeps its state", async () => {
    const mounts: Record<string, number> = {};
    await render(
      <>
        <Reader id='outside' mounts={mounts} />
        <RereadBoundary>
          <Reader id='inside' mounts={mounts} />
        </RereadBoundary>
      </>
    );
    await click($("#outside-type"));
    await click($("#inside-type"));
    await click($("#inside-reread"));
    expect(mounts).toEqual({ outside: 1, inside: 2 });
    expect($("#outside-draft")?.textContent).toBe("typed");
    expect($("#inside-draft")?.textContent).toBe("");
  });

  it("nested, re-reads only the innermost part that failed", async () => {
    const mounts: Record<string, number> = {};
    await render(
      <RereadBoundary>
        <Reader id='screen' mounts={mounts} />
        <RereadBoundary>
          <Reader id='section' mounts={mounts} />
        </RereadBoundary>
      </RereadBoundary>
    );
    await click($("#section-reread"));
    expect(mounts).toEqual({ screen: 1, section: 2 });
    await click($("#screen-reread"));
    expect(mounts).toEqual({ screen: 2, section: 3 });
  });

  it("offers nothing outside a boundary", async () => {
    const mounts: Record<string, number> = {};
    await render(<Reader id='alone' mounts={mounts} />);
    expect(($("#alone-reread") as HTMLButtonElement).disabled).toBe(true);
  });
});
