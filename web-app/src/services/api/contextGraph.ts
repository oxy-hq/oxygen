import type {
  ContextGraph,
  ContextGraphEdge,
  ContextGraphNode,
  ContextGraphSources,
  Topic,
  View
} from "@/types/contextGraph";
import { apiClient } from "./axios";
import {
  addAgentNodes,
  type GraphAgent,
  linkAutomationAgentSteps,
  sqlPathsInAgentContext
} from "./contextGraphAgents";
import { DatabaseService } from "./database";

type AutomationSource = {
  name: string;
  path: string;
  description?: string;
  tasks: unknown[];
};

type ProjectSources = {
  views: View[];
  topics: Topic[];
  agents: GraphAgent[];
  sqlQueries: Array<{ name: string; path: string }>;
  automations: AutomationSource[];
};

/** The file name of `path` with `suffix` removed. */
const stem = (path: string, suffix: RegExp): string =>
  (path.split("/").pop() ?? path).replace(suffix, "") || "unknown";

export class ContextGraphService {
  /**
   * Builds the complete context graph by fetching all assets and computing linkages
   */
  static async getContextGraph(projectId: string): Promise<ContextGraph> {
    // Fetch all necessary data in parallel. The artifacts come from the
    // compiled revision; the page no longer reads workspace files.
    const [databases, { views, topics, agents, sqlQueries, automations }] = await Promise.all([
      DatabaseService.listDatabases(projectId),
      ContextGraphService.loadProjectSources(projectId)
    ]);

    // Build nodes
    const nodes: ContextGraphNode[] = [];
    const edges: ContextGraphEdge[] = [];

    // Add table nodes.
    //
    // `datasets` is `null` when the answering instance could not look, and `{}`
    // when it looked and found none — see the field's doc in types/database.ts,
    // which says only the second may be rendered as "none configured". Treating
    // null as `{}` produced zero table nodes either way, so the overview simply
    // omitted its Tables row and the page looked complete while being short a
    // whole category. Record the distinction instead of erasing it.
    let tablesUnknown = false;
    databases.forEach((db) => {
      if (db.datasets == null) {
        tablesUnknown = true;
        return;
      }
      Object.entries(db.datasets).forEach(([dataset, tables]) => {
        Object.keys(tables).forEach((table) => {
          const tableId = `table:${db.name}.${dataset}.${table}`;
          nodes.push({
            id: tableId,
            type: "table",
            label: table,
            data: {
              name: table,
              database: db.name,
              description: `Table in ${db.name}.${dataset}`,
              metadata: {
                dataset,
                dialect: db.dialect
              }
            }
          });
        });
      });
    });

    // Add view nodes and link to tables
    views.forEach((view) => {
      const viewId = `view:${view.path}`;
      nodes.push({
        id: viewId,
        type: "view",
        label: view.name,
        data: {
          name: view.name,
          path: view.path,
          description: view.description,
          datasource: view.datasource,
          metadata: {
            dimensions: view.dimensions?.length || 0,
            measures: view.measures?.length || 0
          }
        }
      });

      // Link view to its source table
      // Try to find the table node by matching datasource and table name
      const tableNode = nodes.find(
        (n) =>
          n.type === "table" && n.data.database === view.datasource && n.data.name === view.table
      );

      if (tableNode) {
        edges.push({
          id: `${viewId}->${tableNode.id}`,
          source: viewId,
          target: tableNode.id,
          label: "uses",
          type: "uses"
        });
      }
    });

    // Track entities globally (deduplicated by name)
    const entityMap = new Map<string, string>(); // entityName -> entityId

    // Add entity nodes from views
    views.forEach((view) => {
      const viewId = `view:${view.path}`;

      // Parse entities from the view's entities array
      if (view.entities && Array.isArray(view.entities)) {
        view.entities.forEach((entity) => {
          const entityName = entity.name;
          const entityId = `entity:${entityName}`;

          // Create or reference entity node (deduplicated by name)
          if (!entityMap.has(entityName)) {
            entityMap.set(entityName, entityId);
            nodes.push({
              id: entityId,
              type: "entity",
              label: entityName,
              data: {
                name: entityName,
                metadata: {
                  type: entity.type,
                  description: entity.description,
                  keys: entity.keys
                }
              }
            });
          }

          edges.push({
            id: `${entityId}->${viewId}`,
            source: entityId,
            target: viewId,
            label: "defined in",
            type: "uses"
          });
        });
      }
    });

    // Add topic nodes and link to views
    topics.forEach((topic) => {
      const topicId = `topic:${topic.path}`;
      nodes.push({
        id: topicId,
        type: "topic",
        label: topic.name,
        data: {
          name: topic.name,
          path: topic.path,
          description: topic.description,
          metadata: {
            views: topic.views,
            base_view: topic.base_view
          }
        }
      });

      topic.views.forEach((viewName) => {
        const viewNode = nodes.find((n) => n.type === "view" && n.data.name === viewName);
        if (viewNode) {
          edges.push({
            id: `${topicId}->${viewNode.id}`,
            source: topicId,
            target: viewNode.id,
            label: "contains",
            type: "contains"
          });
        }
      });
    });

    // Add procedure and automation nodes and link to their dependencies
    automations.forEach((automation) => {
      const isProcedure =
        automation.path.endsWith(".procedure.yml") || automation.path.endsWith(".procedure.yaml");
      const isAutomation = automation.path.endsWith(".automation.yml");

      let nodeId: string;
      let nodeType: "procedure" | "workflow" | "automation";

      if (isProcedure) {
        nodeId = `procedure:${automation.path}`;
        nodeType = "procedure";
      } else if (isAutomation) {
        nodeId = `automation:${automation.path}`;
        nodeType = "automation";
      } else {
        nodeId = `workflow:${automation.path}`;
        nodeType = "workflow";
      }

      nodes.push({
        id: nodeId,
        type: nodeType,
        label: automation.name,
        data: {
          name: automation.name,
          path: automation.path,
          description: automation.description,
          metadata: {
            tasks: automation.tasks
          }
        }
      });

      // Analyze automation/app tasks to find dependencies
      ContextGraphService.extractAutomationDependencies(automation.tasks, nodeId, nodes, edges);
    });

    // Helper function to check if a path matches a pattern (with wildcards)
    const matchesPattern = (path: string, pattern: string): boolean => {
      if (path === pattern) return true;
      if (!pattern.includes("*")) return false;
      const regexPattern = pattern.replace(/\*/g, ".*");
      return new RegExp(`^${regexPattern}$`).test(path);
    };

    const referencedSqlQueries = new Set<string>();

    // Helper to check tasks for SQL query references
    const checkTasksForSqlReferences = (tasks: unknown[]): void => {
      if (!Array.isArray(tasks)) return;
      tasks.forEach((task) => {
        if (typeof task !== "object" || task === null) return;
        const taskObj = task as Record<string, unknown>;

        if (taskObj.type === "execute_sql" && typeof taskObj.sql_file === "string") {
          sqlQueries.forEach((query) => {
            if (matchesPattern(query.path, taskObj.sql_file as string)) {
              referencedSqlQueries.add(query.path);
            }
          });
        }

        // Recursively check nested tasks
        if (Array.isArray(taskObj.tasks)) {
          checkTasksForSqlReferences(taskObj.tasks);
        }
      });
    };

    automations.forEach((automation) => {
      checkTasksForSqlReferences(automation.tasks);
    });

    for (const path of sqlPathsInAgentContext(agents, sqlQueries)) {
      referencedSqlQueries.add(path);
    }

    // Add only referenced SQL query nodes
    sqlQueries.forEach((query) => {
      if (referencedSqlQueries.has(query.path)) {
        const queryId = `sql_query:${query.path}`;
        nodes.push({
          id: queryId,
          type: "sql_query",
          label: query.name,
          data: {
            name: query.name,
            path: query.path,
            metadata: {}
          }
        });
      }
    });

    // Agent nodes, linked to what their `context:` names, and automations
    // linked to the agents their steps run.
    addAgentNodes(agents, nodes, edges);
    linkAutomationAgentSteps(automations, agents, nodes, edges);

    return { nodes, edges, tablesUnknown };
  }

  /**
   * The served revision's compiled artifacts, in the shapes the graph builder
   * reads. Each `definition` is the parsed YAML of the file at `path`, so this
   * reads exactly the fields the old per-file YAML parse did.
   */
  private static async loadProjectSources(projectId: string): Promise<ProjectSources> {
    const sources = await ContextGraphService.fetchSources(projectId);
    const field = <T>(def: Record<string, unknown>, key: string) => def[key] as T | undefined;

    const views: View[] = sources.views.map(({ path, definition: d }) => ({
      name: field<string>(d, "name") ?? stem(path, /\.view\.ya?ml$/),
      path,
      description: field<string>(d, "description"),
      datasource: field<string>(d, "datasource") ?? "",
      table: field<string>(d, "table") ?? "",
      entities: field<View["entities"]>(d, "entities"),
      dimensions: field<View["dimensions"]>(d, "dimensions"),
      measures: field<View["measures"]>(d, "measures")
    }));

    const topics: Topic[] = sources.topics.map(({ path, definition: d }) => ({
      name: field<string>(d, "name") ?? stem(path, /\.topic\.ya?ml$/),
      path,
      description: field<string>(d, "description"),
      views: field<string[]>(d, "views") ?? [],
      base_view: field<string>(d, "base_view"),
      default_filters: field<Topic["default_filters"]>(d, "default_filters")
    }));

    // The agent's `context:` as plain globs: a string entry is one, and an
    // object entry (`{ name, type, src }`) contributes its `src` list.
    const agents: GraphAgent[] = sources.agents.map(({ name, path, definition: d }) => ({
      name,
      path,
      description: field<string>(d, "description"),
      model: field<{ ref?: string }>(d, "llm")?.ref ?? field<string>(d, "model"),
      context: (field<unknown[]>(d, "context") ?? []).flatMap((entry) => {
        if (typeof entry === "string") return [entry];
        const src = (entry as { src?: unknown } | null)?.src;
        return Array.isArray(src) ? src.filter((s): s is string => typeof s === "string") : [];
      })
    }));

    const automations: AutomationSource[] = sources.automations.map(
      ({ name, path, definition: d }) => ({
        name: field<string>(d, "name") ?? name,
        path,
        description: field<string>(d, "description"),
        tasks: field<unknown[]>(d, "tasks") ?? []
      })
    );

    const sqlQueries = sources.sql_queries.map((path) => ({
      name: stem(path, /\.sql$/),
      path
    }));

    return { views, topics, agents, sqlQueries, automations };
  }

  /** `GET /context-graph/sources` — FleetOk, read from the served revision. */
  static async fetchSources(projectId: string): Promise<ContextGraphSources> {
    const { data } = await apiClient.get<ContextGraphSources>(
      `/${projectId}/context-graph/sources`
    );
    return data;
  }

  /**
   * Extracts dependencies from automation tasks
   */
  private static extractAutomationDependencies(
    tasks: unknown[],
    automationId: string,
    nodes: ContextGraphNode[],
    edges: ContextGraphEdge[]
  ): void {
    if (!Array.isArray(tasks)) return;

    // eslint-disable-next-line sonarjs/cognitive-complexity
    tasks.forEach((task) => {
      // Type guard to ensure task is an object with properties
      if (typeof task !== "object" || task === null) return;

      const taskObj = task as Record<string, unknown>;

      if (taskObj.type === "semantic_query" && typeof taskObj.topic === "string") {
        const topicNode = nodes.find((n) => n.type === "topic" && n.data.name === taskObj.topic);
        if (topicNode) {
          edges.push({
            id: `${automationId}->${topicNode.id}`,
            source: automationId,
            target: topicNode.id,
            label: "uses",
            type: "uses"
          });
        }

        // Also link to views referenced in dimensions/measures
        const viewNames = new Set<string>();

        // Extract view names from dimensions
        if (Array.isArray(taskObj.dimensions)) {
          taskObj.dimensions.forEach((dim) => {
            if (typeof dim === "string" && dim.includes(".")) {
              const viewName = dim.split(".")[0];
              viewNames.add(viewName);
            }
          });
        }

        // Extract view names from measures
        if (Array.isArray(taskObj.measures)) {
          taskObj.measures.forEach((measure) => {
            if (typeof measure === "string" && measure.includes(".")) {
              const viewName = measure.split(".")[0];
              viewNames.add(viewName);
            }
          });
        }

        // Link to views
        viewNames.forEach((viewName) => {
          const viewNode = nodes.find((n) => n.type === "view" && n.data.name === viewName);
          if (viewNode) {
            edges.push({
              id: `${automationId}->${viewNode.id}`,
              source: automationId,
              target: viewNode.id,
              label: "queries",
              type: "uses"
            });
          }
        });
      }

      // Link to SQL query files
      if (taskObj.type === "execute_sql" && typeof taskObj.sql_file === "string") {
        const queryNode = nodes.find(
          (n) => n.type === "sql_query" && n.data.path === taskObj.sql_file
        );
        if (queryNode) {
          edges.push({
            id: `${automationId}->${queryNode.id}`,
            source: automationId,
            target: queryNode.id,
            label: "uses",
            type: "uses"
          });
        }
      }

      // Link to other automations
      if (taskObj.type === "workflow" && typeof taskObj.workflow_ref === "string") {
        const subAutomationNode = nodes.find(
          (n) => n.type === "workflow" && n.data.path === taskObj.workflow_ref
        );
        if (subAutomationNode) {
          edges.push({
            id: `${automationId}->${subAutomationNode.id}`,
            source: automationId,
            target: subAutomationNode.id,
            label: "uses",
            type: "uses"
          });
        }
      }

      // Recursively handle nested tasks
      if (Array.isArray(taskObj.tasks)) {
        ContextGraphService.extractAutomationDependencies(
          taskObj.tasks,
          automationId,
          nodes,
          edges
        );
      }
    });
  }
}
