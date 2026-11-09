import { parse } from "yaml";
import { encodeBase64 } from "@/libs/encoding";
import type {
  ContextGraph,
  ContextGraphEdge,
  ContextGraphNode,
  Topic,
  View
} from "@/types/contextGraph";
import type { FileTreeModel } from "@/types/file";
import {
  addAgentNodes,
  linkAutomationAgentSteps,
  loadGraphAgents,
  sqlPathsInAgentContext
} from "./contextGraphAgents";
import { DatabaseService } from "./database";
import { FileService } from "./files";

export class ContextGraphService {
  /**
   * Builds the complete context graph by fetching all assets and computing linkages
   */
  static async getContextGraph(projectId: string, branchName: string): Promise<ContextGraph> {
    // Fetch all necessary data in parallel
    const [databases, fileTree, agents] = await Promise.all([
      DatabaseService.listDatabases(projectId, branchName),
      FileService.getFileTree(projectId, branchName),
      loadGraphAgents(projectId, branchName)
    ]);

    // Parse semantic models, queries and automations from the file tree.
    // Agents come from the agent listing above, not from file names.
    const { views, topics, sqlQueries, automations } = await ContextGraphService.parseProjectFiles(
      projectId,
      branchName,
      fileTree.primary
    );

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
   * Parses project files (views, topics, SQL queries, automations) from the file tree
   */
  private static async parseProjectFiles(
    projectId: string,
    branchName: string,
    fileTree: FileTreeModel[]
  ): Promise<{
    views: View[];
    topics: Topic[];
    sqlQueries: Array<{
      name: string;
      path: string;
    }>;
    automations: Array<{
      name: string;
      path: string;
      description?: string;
      tasks: unknown[];
    }>;
  }> {
    const views: View[] = [];
    const topics: Topic[] = [];
    const sqlQueries: Array<{
      name: string;
      path: string;
    }> = [];
    const automations: Array<{
      name: string;
      path: string;
      description?: string;
      tasks: unknown[];
    }> = [];

    const projectFiles = ContextGraphService.findProjectFiles(fileTree);

    await Promise.all(
      projectFiles.map(async (file) => {
        try {
          const content = await FileService.getFile(projectId, encodeBase64(file.path), branchName);

          if (
            file.type === "view" ||
            file.type === "topic" ||
            file.type === "procedure" ||
            file.type === "workflow" ||
            file.type === "automation"
          ) {
            const parsed = parse(content);

            if (file.type === "view") {
              views.push({
                name: parsed.name,
                path: file.path,
                description: parsed.description,
                datasource: parsed.datasource,
                table: parsed.table,
                entities: parsed.entities,
                dimensions: parsed.dimensions,
                measures: parsed.measures
              });
            } else if (file.type === "topic") {
              topics.push({
                name: parsed.name,
                path: file.path,
                description: parsed.description,
                views: parsed.views || [],
                base_view: parsed.base_view,
                default_filters: parsed.default_filters
              });
            } else if (
              file.type === "procedure" ||
              file.type === "workflow" ||
              file.type === "automation"
            ) {
              const automationName =
                parsed.name ||
                file.path
                  .split("/")
                  .pop()
                  ?.replace(/\.(procedure|workflow|app|automation)\.yml$/, "") ||
                "unknown";
              automations.push({
                name: automationName,
                path: file.path,
                description: parsed.description,
                tasks: parsed.tasks || []
              });
            }
          } else if (file.type === "sql") {
            const queryName = file.path.split("/").pop()?.replace(".sql", "") || "unknown";
            sqlQueries.push({
              name: queryName,
              path: file.path
            });
          }
        } catch (error) {
          console.error(`Failed to parse ${file.path}:`, error);
        }
      })
    );

    return { views, topics, sqlQueries, automations };
  }

  /**
   * Recursively finds all project files (views, topics, SQL, automations) in the file tree
   */
  private static findProjectFiles(
    fileTree: FileTreeModel[],
    basePath = ""
  ): Array<{ path: string; type: string }> {
    const files: Array<{ path: string; type: string }> = [];

    fileTree.forEach((node) => {
      const currentPath = basePath ? `${basePath}/${node.name}` : node.name;

      if (node.is_dir && node.children) {
        files.push(...ContextGraphService.findProjectFiles(node.children, currentPath));
      } else if (!node.is_dir) {
        if (node.name.endsWith(".view.yml") || node.name.endsWith(".view.yaml")) {
          files.push({ path: currentPath, type: "view" });
        } else if (node.name.endsWith(".topic.yml") || node.name.endsWith(".topic.yaml")) {
          files.push({ path: currentPath, type: "topic" });
        } else if (node.name.endsWith(".sql")) {
          files.push({ path: currentPath, type: "sql" });
        } else if (node.name.endsWith(".procedure.yml") || node.name.endsWith(".procedure.yaml")) {
          files.push({ path: currentPath, type: "procedure" });
        } else if (
          node.name.endsWith(".automation.yml") ||
          node.name.endsWith(".automation.yaml")
        ) {
          files.push({ path: currentPath, type: "automation" });
        }
      }
    });

    return files;
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
