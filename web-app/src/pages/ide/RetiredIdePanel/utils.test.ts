import { describe, expect, it } from "vitest";
import { gitCloneCommand, oxycCloneCommand, shellQuote, vscodeCloneUrl } from "./utils";

describe("RetiredIdePanel commands", () => {
  it("quotes a branch carrying shell metacharacters", () => {
    expect(gitCloneCommand("https://github.com/acme/repo.git", "x; rm -rf ~")).toBe(
      "git clone --branch 'x; rm -rf ~' -- 'https://github.com/acme/repo.git'"
    );
  });

  it("quotes the remote with no branch, and escapes an embedded quote", () => {
    expect(gitCloneCommand("https://h/a'b.git", null)).toBe("git clone -- 'https://h/a'\\''b.git'");
    expect(shellQuote("it's")).toBe("'it'\\''s'");
  });

  it("quotes the workspace id and encodes the VS Code link", () => {
    expect(oxycCloneCommand("7a4e")).toBe("oxyc workspace clone '7a4e'");
    expect(vscodeCloneUrl("https://h/r.git?x=1")).toBe(
      "vscode://vscode.git/clone?url=https%3A%2F%2Fh%2Fr.git%3Fx%3D1"
    );
  });
});
