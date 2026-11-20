// @vitest-environment jsdom
//
// The attachments field: picking files hands them over and clears the input, each tile carries
// its own state (uploading words, a failed tile's Retry), a remove hands focus on rather than to
// <body>, the phone shows the whole name and the reason, and at its limit the picker says so,
// refuses, and keeps focus.

import { afterEach, describe, expect, it, vi } from "vitest";
import { $, $$, click, fire, render } from "../ui.harness";
import { type AttachmentFile, AttachmentsField, formatFileSize } from "./AttachmentsField";

afterEach(() => {
  document.body.innerHTML = "";
});

const NAME = "IMG_20261104_093012_packing_slip.jpg";
const REASON = `${NAME} is too large: the limit is 10 MB per file`;
const file = (id: string, extra: Partial<AttachmentFile> = {}): AttachmentFile => ({
  id,
  name: `${id}.pdf`,
  size: 2048,
  previewUrl: null,
  status: "uploaded",
  ...extra
});
const noop = () => {};
const picker = () => $("input[type='file']") as HTMLInputElement;
const removes = () => $$("[data-remove]") as HTMLButtonElement[];

describe("AttachmentsField", () => {
  it("hands picked files over and clears the input, so the same file can be picked again", async () => {
    const onAdd = vi.fn();
    await render(<AttachmentsField files={[]} onAdd={onAdd} onRemove={noop} onRetry={noop} />);
    expect($("label")?.textContent).toBe("Add photos or files");
    const input = picker();
    expect(input.multiple).toBe(true);
    expect(input.accept).toBe("image/*,application/pdf");
    const picked = { length: 1, item: () => null } as unknown as FileList;
    const cleared = vi.fn();
    Object.defineProperty(input, "files", { configurable: true, value: picked });
    Object.defineProperty(input, "value", { configurable: true, get: () => "", set: cleared });
    await fire(input, new Event("change", { bubbles: true }));
    expect(onAdd).toHaveBeenCalledWith(picked);
    expect(cleared).toHaveBeenLastCalledWith("");
  });

  it("draws each tile's name and size, and removes by a name that says which", async () => {
    const onRemove = vi.fn();
    await render(
      <AttachmentsField
        files={[file("a", { size: 12_000_000 })]}
        onAdd={noop}
        onRemove={onRemove}
        onRetry={noop}
      />
    );
    expect(document.body.textContent).toContain("a.pdf");
    expect(document.body.textContent).toContain("11 MB");
    expect($("label")?.textContent).toBe("Add more");
    expect(removes()[0].getAttribute("aria-label")).toBe("Remove a.pdf");
    await click(removes()[0]);
    expect(onRemove).toHaveBeenCalledWith("a");
  });

  it("hands focus to the next tile's remove, then the one before, then the picker", async () => {
    const all = [file("a"), file("b"), file("c")];
    const page = await render(
      <AttachmentsField files={all} onAdd={noop} onRemove={noop} onRetry={noop} />
    );
    removes()[1].focus();
    await click(removes()[1]);
    await page.rerender(
      <AttachmentsField files={[all[0], all[2]]} onAdd={noop} onRemove={noop} onRetry={noop} />
    );
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Remove c.pdf");
    await click(removes()[1]);
    await page.rerender(
      <AttachmentsField files={[all[0]]} onAdd={noop} onRemove={noop} onRetry={noop} />
    );
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Remove a.pdf");
    await click(removes()[0]);
    await page.rerender(
      <AttachmentsField files={[]} onAdd={noop} onRemove={noop} onRetry={noop} />
    );
    expect(document.activeElement).toBe(picker());
  });

  it("says Uploading on a tile in flight, and offers Retry with the reason on a failed one", async () => {
    const onRetry = vi.fn();
    await render(
      <AttachmentsField
        files={[
          file("up", { status: "uploading" }),
          file("bad", { name: NAME, status: "failed", error: REASON })
        ]}
        onAdd={noop}
        onRemove={noop}
        onRetry={onRetry}
        phone={false}
      />
    );
    expect($(".oxy-ui-attachments__veil")?.textContent).toBe("Uploading… up.pdf");
    const retry = $(".oxy-ui-attachments__retry") as HTMLButtonElement;
    expect(retry.title).toBe(REASON);
    expect(retry.textContent).toBe(`Upload of ${NAME} failed: ${REASON}. Retry.`);
    expect($(".oxy-ui-attachments__failed")?.getAttribute("aria-hidden")).toBe("true");
    await click(retry);
    expect(onRetry).toHaveBeenCalledWith("bad");
    // Desk: the name is cut with its hover title, and the reason is only in Retry.
    expect($(".oxy-ui-attachments__name--cut[title]")).not.toBeNull();
    expect($(".oxy-ui-attachments__reason")).toBeNull();
  });

  it("on a phone shows the whole name and the reason, with no hover title", async () => {
    await render(
      <AttachmentsField
        files={[file("bad", { name: NAME, status: "failed", error: REASON })]}
        onAdd={noop}
        onRemove={noop}
        onRetry={noop}
        phone
      />
    );
    expect($(".oxy-ui-attachments__name--wrap")?.textContent).toBe(NAME);
    expect($(`[title="${NAME}"]`)).toBeNull();
    expect($(".oxy-ui-attachments__reason")?.textContent).toBe(REASON);
  });

  it("at its limit says so, refuses a pick and keeps focus on the picker", async () => {
    const page = await render(
      <AttachmentsField files={[file("a")]} onAdd={noop} onRemove={noop} onRetry={noop} max={2} />
    );
    const open = new MouseEvent("click", { bubbles: true, cancelable: true });
    await fire(picker(), open);
    expect(open.defaultPrevented, "under the limit the picker opens").toBe(false);
    picker().focus();
    await page.rerender(
      <AttachmentsField
        files={[file("a"), file("b")]}
        onAdd={noop}
        onRemove={noop}
        onRetry={noop}
        max={2}
      />
    );
    expect($("label")?.textContent).toBe("Limit reached (2 files)");
    expect(picker().getAttribute("aria-disabled")).toBe("true");
    expect(picker().disabled).toBe(false);
    expect(document.activeElement).toBe(picker());
    const refused = new MouseEvent("click", { bubbles: true, cancelable: true });
    await fire(picker(), refused);
    expect(refused.defaultPrevented).toBe(true);
    expect($("label")?.hasAttribute("data-disabled")).toBe(true);
  });

  it("locks every control while disabled, and announces a refused file", async () => {
    await render(
      <AttachmentsField
        files={[file("bad", { status: "failed", error: "Too big" })]}
        onAdd={noop}
        onRemove={noop}
        onRetry={noop}
        disabled
        notice='Only 20 files fit on one report'
      />
    );
    expect(picker().disabled).toBe(true);
    for (const b of $$("button")) expect((b as HTMLButtonElement).disabled).toBe(true);
    expect($("[role='alert']")?.textContent).toBe("Only 20 files fit on one report");
  });

  it("writes sizes as warehouse did", () => {
    expect(formatFileSize(512)).toBe("512 B");
    expect(formatFileSize(10_000)).toBe("9.8 KB");
    expect(formatFileSize(3.4 * 1024 * 1024)).toBe("3.4 MB");
    expect(formatFileSize(-1)).toBe("");
  });
});
