// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DatabaseInfo } from "@/types/database";
import { AgentService } from "./agents";
import { ContextGraphService } from "./contextGraph";
import { DatabaseService } from "./database";
import { FileService } from "./files";

// A stateless replica answers `datasets: null` for every database, because the
// semantic sync directory lives in the working copy it does not have. The ide
// answers `{}` for a database it looked at and found nothing synced for. Both
// produce zero table nodes, so the graph alone cannot tell them apart — which
// is how "could not look" used to render as a clean, complete-looking page with
// its Tables row quietly absent. These tests pin the distinction.
//
// The payloads below are the real shapes observed on the docker fleet at the
// same instant: ide :3010 returned `{}` for `training`, both serve replicas
// returned null for every database.

const db = (name: string, datasets: DatabaseInfo["datasets"], synced: boolean): DatabaseInfo => ({
  name,
  dialect: "duckdb",
  db_type: "duckdb",
  datasets,
  synced
});

const SERVE_REPLICA_ANSWER: DatabaseInfo[] = [
  db("primary_database", null, false),
  db("training", null, false)
];

const IDE_ANSWER: DatabaseInfo[] = [
  db(
    "primary_database",
    {
      main: {
        orders: { table: "orders", database: "primary_database" }
      }
    },
    true
  ),
  // Looked, and there is genuinely nothing synced. Not the same as null.
  db("training", {}, false)
];

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(AgentService, "listAgents").mockResolvedValue([]);
  vi.spyOn(FileService, "getFileTree").mockResolvedValue({
    primary: []
  } as unknown as Awaited<ReturnType<typeof FileService.getFileTree>>);
});

describe("ContextGraphService.getContextGraph — datasets null vs {}", () => {
  it("flags tablesUnknown when a database answers null (replica could not look)", async () => {
    vi.spyOn(DatabaseService, "listDatabases").mockResolvedValue(SERVE_REPLICA_ANSWER);

    const graph = await ContextGraphService.getContextGraph("ws", "main");

    expect(graph.tablesUnknown).toBe(true);
    expect(graph.nodes.filter((n) => n.type === "table")).toHaveLength(0);
  });

  it("does not flag tablesUnknown when every database answered, even with none synced", async () => {
    vi.spyOn(DatabaseService, "listDatabases").mockResolvedValue(IDE_ANSWER);

    const graph = await ContextGraphService.getContextGraph("ws", "main");

    expect(graph.tablesUnknown).toBe(false);
    // `training: {}` contributes nothing, `primary_database` contributes one.
    expect(graph.nodes.filter((n) => n.type === "table").map((n) => n.id)).toEqual([
      "table:primary_database.main.orders"
    ]);
  });

  it("separates the two states that both yield zero table nodes", async () => {
    vi.spyOn(DatabaseService, "listDatabases").mockResolvedValue([db("training", {}, false)]);
    const lookedAndFoundNone = await ContextGraphService.getContextGraph("ws", "main");

    vi.spyOn(DatabaseService, "listDatabases").mockResolvedValue([db("training", null, false)]);
    const couldNotLook = await ContextGraphService.getContextGraph("ws", "main");

    // Identical node counts — this is precisely why the flag has to exist.
    expect(lookedAndFoundNone.nodes.filter((n) => n.type === "table")).toHaveLength(0);
    expect(couldNotLook.nodes.filter((n) => n.type === "table")).toHaveLength(0);
    expect(lookedAndFoundNone.tablesUnknown).toBe(false);
    expect(couldNotLook.tablesUnknown).toBe(true);
  });
});
