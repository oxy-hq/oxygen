import { describe, expect, it } from "vitest";
import ROUTES from "@/libs/utils/routes";
import { factoryPageIn } from "./factoryPageIn";

/** Moving to another workspace's Factory keeps the section, never an id inside it. */

const from = ROUTES.ORG("acme").WORKSPACE("ws-1").IDE.ROOT;
const to = ROUTES.ORG("acme").WORKSPACE("ws-2").IDE;
const pageFor = (pathname: string) => factoryPageIn(to, from, pathname);

describe("factoryPageIn", () => {
  it("keeps the section", () => {
    expect(pageFor(`${from}/semantic`)).toBe("/acme/workspaces/ws-2/ide/semantic");
    expect(pageFor(`${from}/world-model`)).toBe("/acme/workspaces/ws-2/ide/world-model");
    expect(pageFor(`${from}/database`)).toBe("/acme/workspaces/ws-2/ide/database");
  });

  it("drops what belongs to the workspace being left", () => {
    expect(pageFor(`${from}/files/c3JjL2EueW1s`)).toBe("/acme/workspaces/ws-2/ide/files");
    expect(pageFor(`${from}/experiments/price-test`)).toBe("/acme/workspaces/ws-2/ide/experiments");
    expect(pageFor(`${from}/edge/boxes/box-7`)).toBe("/acme/workspaces/ws-2/ide/edge");
    expect(pageFor(`${from}/coordinator/runs/run-9`)).toBe(
      "/acme/workspaces/ws-2/ide/coordinator/runs"
    );
    expect(pageFor(`${from}/observability/traces/abc`)).toBe(
      "/acme/workspaces/ws-2/ide/observability/traces"
    );
  });

  it("keeps the coordinator's and observability's tab, or opens their first", () => {
    expect(pageFor(`${from}/coordinator/queue`)).toBe(
      "/acme/workspaces/ws-2/ide/coordinator/queue"
    );
    expect(pageFor(`${from}/coordinator`)).toBe("/acme/workspaces/ws-2/ide/coordinator/overview");
    expect(pageFor(`${from}/observability`)).toBe("/acme/workspaces/ws-2/ide/observability/traces");
  });

  it("opens the Factory's landing for anything else", () => {
    expect(pageFor(from)).toBe("/acme/workspaces/ws-2/ide");
    expect(pageFor(`${from}/something-new`)).toBe("/acme/workspaces/ws-2/ide");
    // An org slug that is itself "ide" is not mistaken for the section.
    const odd = ROUTES.ORG("ide").WORKSPACE("ws-1").IDE.ROOT;
    expect(factoryPageIn(to, odd, `${odd}/semantic`)).toBe("/acme/workspaces/ws-2/ide/semantic");
  });
});
