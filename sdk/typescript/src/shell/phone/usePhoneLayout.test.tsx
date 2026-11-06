// @vitest-environment jsdom
//
// `usePhoneLayout` decides which view an app renders, so it must answer from
// the first render (no desktop flash on a phone), follow a rotation, and say
// "desktop" where there is no matchMedia at all.

import { renderToString } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { render, stubMatchMedia } from "./phone.harness";
import { PHONE_LAYOUT_QUERY, usePhoneLayout } from "./usePhoneLayout";

function Probe() {
  return <span data-testid='probe'>{usePhoneLayout() ? "phone" : "desktop"}</span>;
}

const probe = () => document.querySelector('[data-testid="probe"]')?.textContent;

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

describe("usePhoneLayout", () => {
  it("asks for a phone upright or on its side", () => {
    expect(PHONE_LAYOUT_QUERY).toBe(
      "(max-width: 639.98px), (pointer: coarse) and (max-height: 500px)"
    );
  });

  it("answers from the first render", async () => {
    const media = stubMatchMedia(true);
    vi.stubGlobal("matchMedia", media.matchMedia);
    const page = await render(<Probe />);
    expect(probe()).toBe("phone");
    expect(media.queries).toContain(PHONE_LAYOUT_QUERY);
    await page.unmount();
  });

  it("follows a change of the query, and stops listening on unmount", async () => {
    const media = stubMatchMedia(false);
    vi.stubGlobal("matchMedia", media.matchMedia);
    const page = await render(<Probe />);
    expect(probe()).toBe("desktop");
    await media.set(true);
    expect(probe()).toBe("phone");
    await media.set(false);
    expect(probe()).toBe("desktop");
    await page.unmount();
    expect(media.listeners.size).toBe(0);
  });

  it("is desktop without matchMedia, and on the server", async () => {
    vi.stubGlobal("matchMedia", undefined);
    const page = await render(<Probe />);
    expect(probe()).toBe("desktop");
    await page.unmount();
    expect(renderToString(<Probe />)).toContain("desktop");
  });
});
