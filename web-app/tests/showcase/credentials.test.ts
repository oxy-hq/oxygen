import { describe, expect, it } from "vitest";
import { expandSecrets } from "../agentic/runner/secrets";
import { credential, ghAuthEnv, takeCredentials } from "./credentials";

/** `${name}` as a step would carry it — written this way so it is not mistaken for a template. */
const ph = (name: string) => `\${${name}}`;

describe("takeCredentials", () => {
  it("leaves nothing in the environment for a step to expand", () => {
    const env: NodeJS.ProcessEnv = {
      ANTHROPIC_API_KEY: "sk-ant-secret",
      SLACK_BOT_TOKEN: "xoxb-secret",
      GH_TOKEN: "ghs_secret",
      OXY_BASE_URL: "http://127.0.0.1:3000"
    };
    takeCredentials(env);
    expect(env).toEqual({ OXY_BASE_URL: "http://127.0.0.1:3000" });
    expect(credential("SLACK_BOT_TOKEN")).toBe("xoxb-secret");
    expect(credential("ANTHROPIC_API_KEY")).toBe("sk-ant-secret");
    expect(ghAuthEnv()).toEqual({ GH_TOKEN: "ghs_secret" });
  });

  // The whole point, end to end: the runner's own expansion, over a step a
  // prompt-injected planner could have written.
  it("makes the runner refuse a step that names a credential", () => {
    process.env.SLACK_BOT_TOKEN = "xoxb-secret";
    expect(expandSecrets(`Type '${ph("SLACK_BOT_TOKEN")}'`)).toBe("Type 'xoxb-secret'");
    takeCredentials();
    expect(() => expandSecrets(`Type '${ph("SLACK_BOT_TOKEN")}'`)).toThrow(/SLACK_BOT_TOKEN/);
    expect(credential("SLACK_BOT_TOKEN")).toBe("xoxb-secret");
  });
});
