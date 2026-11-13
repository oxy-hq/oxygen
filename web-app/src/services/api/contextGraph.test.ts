// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DatabaseInfo } from "@/types/database";
import { ContextGraphService } from "./contextGraph";
import { DatabaseService } from "./database";

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
  vi.spyOn(ContextGraphService, "fetchSources").mockResolvedValue({
    views: [],
    topics: [],
    automations: [],
    agents: [],
    sql_queries: []
  });
});

describe("ContextGraphService.getContextGraph — datasets null vs {}", () => {
  it("flags tablesUnknown when a database answers null (replica could not look)", async () => {
    vi.spyOn(DatabaseService, "listDatabases").mockResolvedValue(SERVE_REPLICA_ANSWER);

    const graph = await ContextGraphService.getContextGraph("ws");

    expect(graph.tablesUnknown).toBe(true);
    expect(graph.nodes.filter((n) => n.type === "table")).toHaveLength(0);
  });

  it("does not flag tablesUnknown when every database answered, even with none synced", async () => {
    vi.spyOn(DatabaseService, "listDatabases").mockResolvedValue(IDE_ANSWER);

    const graph = await ContextGraphService.getContextGraph("ws");

    expect(graph.tablesUnknown).toBe(false);
    // `training: {}` contributes nothing, `primary_database` contributes one.
    expect(graph.nodes.filter((n) => n.type === "table").map((n) => n.id)).toEqual([
      "table:primary_database.main.orders"
    ]);
  });

  it("separates the two states that both yield zero table nodes", async () => {
    vi.spyOn(DatabaseService, "listDatabases").mockResolvedValue([db("training", {}, false)]);
    const lookedAndFoundNone = await ContextGraphService.getContextGraph("ws");

    vi.spyOn(DatabaseService, "listDatabases").mockResolvedValue([db("training", null, false)]);
    const couldNotLook = await ContextGraphService.getContextGraph("ws");

    // Identical node counts — this is precisely why the flag has to exist.
    expect(lookedAndFoundNone.nodes.filter((n) => n.type === "table")).toHaveLength(0);
    expect(couldNotLook.nodes.filter((n) => n.type === "table")).toHaveLength(0);
    expect(lookedAndFoundNone.tablesUnknown).toBe(false);
    expect(couldNotLook.tablesUnknown).toBe(true);
  });
});

// The graph is built from the compiled revision (`GET /context-graph/sources`),
// not from workspace files: each artifact's `definition` is its parsed YAML.
describe("ContextGraphService.getContextGraph — compiled sources", () => {
  it("draws views, topics and automations from their definitions, with their edges", async () => {
    vi.spyOn(DatabaseService, "listDatabases").mockResolvedValue([]);
    vi.spyOn(ContextGraphService, "fetchSources").mockResolvedValue({
      views: [
        {
          name: "orders",
          path: "semantics/views/orders.view.yml",
          definition: { name: "orders", datasource: "db", table: "orders" }
        }
      ],
      topics: [
        {
          name: "sales",
          path: "semantics/topics/sales.topic.yml",
          definition: { name: "sales", views: ["orders"] }
        }
      ],
      automations: [
        {
          name: "daily",
          path: "automations/daily.automation.yml",
          definition: {
            name: "daily",
            tasks: [{ type: "execute_sql", sql_file: "queries/a.sql" }]
          }
        }
      ],
      agents: [],
      sql_queries: ["queries/a.sql", "queries/unused.sql"]
    });

    const graph = await ContextGraphService.getContextGraph("ws");

    const ids = graph.nodes.map((n) => n.id);
    expect(ids).toContain("view:semantics/views/orders.view.yml");
    expect(ids).toContain("topic:semantics/topics/sales.topic.yml");
    expect(ids).toContain("automation:automations/daily.automation.yml");
    // Only a referenced query is drawn.
    expect(ids).toContain("sql_query:queries/a.sql");
    expect(ids).not.toContain("sql_query:queries/unused.sql");
    expect(graph.edges.map((e) => e.id)).toContain(
      "topic:semantics/topics/sales.topic.yml->view:semantics/views/orders.view.yml"
    );
  });
});
