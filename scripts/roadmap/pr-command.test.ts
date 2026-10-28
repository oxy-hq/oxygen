import assert from "node:assert/strict";
import { test } from "node:test";
import { parsePrCreate } from "./pr-command.ts";

const body = "## Why\n\nBecause.\n\nRoadmap: none -- tooling only";

test("a quoted heredoc body, the usual agent form, with title and base", () => {
  const cmd = `gh pr create --title "feat: a thing" --base main --body "$(cat <<'EOF'\n${body}\nEOF\n)"`;
  const p = parsePrCreate(cmd);
  assert.deepEqual(p, {
    found: true,
    title: "feat: a thing",
    base: "main",
    repo: null,
    body,
    bodyFile: null,
    cwd: null,
    cwdUnresolved: false,
    dynamic: false
  });
});

test("a cd the text cannot settle leaves the checkout unknown; a plain one, anywhere, moves it", () => {
  const slot = parsePrCreate(
    `cd "$(just wt-claim fix/x)" && git push -u origin fix/x && gh pr create -t "feat: x" -b "b"`
  );
  assert.equal(slot.found && slot.cwdUnresolved, true);
  assert.equal(
    parsePrCreate(`cd ~/work/slot && gh pr create -t "feat: x" -b "b"`).found && true,
    true
  );
  assert.equal(
    (
      parsePrCreate(`cd ~/work/slot && gh pr create -t "feat: x" -b "b"`) as {
        cwdUnresolved: boolean;
      }
    ).cwdUnresolved,
    true
  );
  assert.equal(
    (parsePrCreate(`cd - && gh pr create -t "feat: x" -b "b"`) as { cwdUnresolved: boolean })
      .cwdUnresolved,
    true
  );
  const later = parsePrCreate(
    `git fetch origin main && cd ../slot-2 && gh pr create -t "feat: x" -b "b"`
  );
  assert.equal(later.found && later.cwd, "../slot-2");
  assert.equal(later.found && later.cwdUnresolved, false);
  const twice = parsePrCreate(`cd /work && cd slot-3 && gh pr create -t "feat: x" -b "b"`);
  assert.equal(twice.found && twice.cwd, "/work/slot-3");
  // A `cd` after the PR opens is not its checkout.
  const after = parsePrCreate(`gh pr create -t "feat: x" -b "b" && cd "$(mktemp -d)"`);
  assert.equal(after.found && after.cwdUnresolved, false);
});

test("flags of earlier commands in the chain are not the PR's", () => {
  const p = parsePrCreate(
    `git commit -F /tmp/msg.txt && git push -u origin x && gh pr create --title 'feat: x' --body-file /tmp/pr.md`
  );
  assert.equal(p.found, true);
  if (p.found) {
    assert.equal(p.title, "feat: x");
    assert.equal(p.bodyFile, "/tmp/pr.md");
    assert.equal(p.body, null);
    assert.equal(p.dynamic, false);
  }
  const fill = parsePrCreate(`git commit -F msg && gh pr create --fill`);
  assert.equal(fill.found && fill.dynamic, true);
  assert.equal(fill.found && fill.bodyFile, null);
});

test("a body file the same command writes is read from the command, not from disk", () => {
  const p = parsePrCreate(
    `cat > /tmp/pr-body.md <<'EOF'\n${body}\nEOF\ngh pr create -t "fix: y" -F /tmp/pr-body.md`
  );
  assert.equal(p.found && p.body, body);
  assert.equal(p.found && p.bodyFile, null);
  // Written from something the text does not settle: dynamic, so the hook fails open.
  const dyn = parsePrCreate(`echo "$BODY" > b.md && gh pr create -t "fix: y" -F b.md`);
  assert.equal(dyn.found && dyn.dynamic, true);
  const stdin = parsePrCreate(`gh pr create -t "fix: y" -F - <<'EOF'\n${body}\nEOF`);
  assert.equal(stdin.found && stdin.body, body);
});

test("a gh pr create mentioned inside a heredoc or a comment opens no PR", () => {
  assert.deepEqual(
    parsePrCreate(`git commit -F - <<'EOF'\nchore: notes\n\nthen gh pr create --fill\nEOF`),
    { found: false }
  );
  assert.deepEqual(parsePrCreate(`cat > doc.md <<EOF\n- Run gh pr create --title x\nEOF`), {
    found: false
  });
  assert.deepEqual(parsePrCreate(`# gh pr create later\ngit status`), { found: false });
  assert.deepEqual(parsePrCreate(`git status`), { found: false });
});

test("a leading cd names the checkout; --repo, --body=, and env assignments are read", () => {
  const p = parsePrCreate(
    `cd /work/slot-3 && GH_TOKEN=x gh pr create --repo oxy-hq/other --title="feat: z" --body="plain body"`
  );
  assert.equal(p.found && p.cwd, "/work/slot-3");
  assert.equal(p.found && p.repo, "oxy-hq/other");
  assert.equal(p.found && p.title, "feat: z");
  assert.equal(p.found && p.body, "plain body");
  const variable = parsePrCreate(`gh pr create --title "$TITLE" --body "x"`);
  assert.equal(variable.found && variable.dynamic, true);
});
