import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { enterpriseServerEnv, seedEnv } from "./backend";

// The environment the runner hands its spawned enterprise server and the seed
// that follows. Both mistakes these pin exit 0 and surface only as flows that
// time out: a flow identity that became staff (bounced to /admin), and a seed
// written to a database the server is not reading (every flow on /onboarding).

describe("enterprise spawn environment", () => {
  let savedFlowEmail: string | undefined;

  beforeEach(() => {
    savedFlowEmail = process.env.OXY_FLOW_EMAIL;
    delete process.env.OXY_FLOW_EMAIL;
  });
  afterEach(() => {
    if (savedFlowEmail === undefined) delete process.env.OXY_FLOW_EMAIL;
    else process.env.OXY_FLOW_EMAIL = savedFlowEmail;
  });

  it("allows the flow identity to dev-login and keeps the caller's list", () => {
    const env = enterpriseServerEnv({ OXY_DEV_LOGIN_EMAILS: "a@x.test" });
    expect(env.OXY_DEV_LOGIN_EMAILS).toBe("a@x.test,flow@oxy.local");
  });

  it("never lets the flow identity reach the server's OXY_GLOBAL_ADMINS", () => {
    const env = enterpriseServerEnv({ OXY_GLOBAL_ADMINS: "Flow@oxy.local, me@oxy.test" });
    expect(env.OXY_GLOBAL_ADMINS).toBe("me@oxy.test");
  });

  it("refuses a flow identity that is the Global Owner", () => {
    process.env.OXY_FLOW_EMAIL = "me@oxy.test";
    expect(() => enterpriseServerEnv({ OXY_OWNER: "me@oxy.test" })).toThrow(/OXY_OWNER/);
  });

  it("seeds the database `oxy start` serves, not an inherited one", () => {
    const env = seedEnv({ OXY_DATABASE_URL: "postgresql://u:p@localhost:5432/other" });
    expect(env.OXY_DATABASE_URL).toBe("postgresql://postgres:postgres@localhost:15432/oxy");
  });

  it("binds the caller's owners and the flow identity in the seed", () => {
    const env = seedEnv({ OXY_GLOBAL_ADMINS: "me@oxy.test,flow@oxy.local" });
    expect(env.OXY_GLOBAL_ADMINS).toBe("me@oxy.test,flow@oxy.local");
  });
});
