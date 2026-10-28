import { parse as parseYaml } from "yaml";

// ── YAML types ───────────────────────────────────────────────────────────────

export type ViewField = {
  name: string;
  type?: string;
  description?: string;
  expr?: string;
};

export type SemanticView = {
  name?: string;
  description?: string;
  datasource?: string;
  table?: string;
  dimensions?: ViewField[];
  measures?: ViewField[];
};

type AutomationTask = {
  name: string;
  type: string;
  database?: string;
  sql_query?: string;
  sql_file?: string;
  template?: string;
};

export type AutomationConfig = {
  name?: string;
  description?: string;
  tasks?: AutomationTask[];
};

export type TopicConfig = {
  name?: string;
  description?: string;
  views?: string[];
};

type AwTransition = {
  type: string;
  database?: string;
};

export type AwConfig = {
  model?: string;
  start?: { mode?: string; instruction?: string; next?: string[] };
  end?: { output_artifact?: string; mode?: string };
  transitions?: AwTransition[];
};

type TestCase = {
  name?: string;
  prompt: string;
  expected: string;
  tags?: string[];
  tool?: string;
};

type TestSettings = {
  concurrency?: number;
  runs?: number;
  judge_model?: string;
};

export type TestFileConfig = {
  name?: string;
  target?: string;
  settings?: TestSettings;
  cases?: TestCase[];
};

// ── Diff types ───────────────────────────────────────────────────────────────

export type FieldDiffStatus = "added" | "removed" | "modified" | "unchanged";

export type FieldDiff = {
  name: string;
  status: FieldDiffStatus;
  kind: "dim" | "measure";
  field: ViewField;
  oldField?: ViewField;
  changes?: string[];
};

type AppItemKind = "task" | "display" | "tool" | "context" | "view" | "transition" | "test_case";

export type AppItemDiff = {
  key: string;
  status: FieldDiffStatus;
  kind: AppItemKind;
  label: string;
  title: string;
  subtitle?: string;
  changes?: string[];
  children?: AppItemDiff[];
};

export type ItemGroup = { label: string; items: AppItemDiff[] };

// ── Parse helpers ─────────────────────────────────────────────────────────────

export function tryParseView(content: string): SemanticView | null {
  try {
    const parsed = parseYaml(content);
    if (parsed && typeof parsed === "object" && (parsed.dimensions || parsed.measures)) {
      return parsed as SemanticView;
    }
  } catch {
    // not valid YAML
  }
  return null;
}

export function tryParseAutomation(content: string): AutomationConfig | null {
  try {
    const parsed = parseYaml(content);
    if (parsed && typeof parsed === "object" && Array.isArray(parsed.tasks)) {
      return parsed as AutomationConfig;
    }
  } catch {
    // not valid YAML
  }
  return null;
}

export function tryParseTopic(content: string): TopicConfig | null {
  try {
    const parsed = parseYaml(content);
    if (parsed && typeof parsed === "object" && Array.isArray(parsed.views)) {
      return parsed as TopicConfig;
    }
  } catch {
    // not valid YAML
  }
  return null;
}

export function tryParseAw(content: string): AwConfig | null {
  try {
    const parsed = parseYaml(content);
    if (parsed && typeof parsed === "object" && (parsed.transitions || parsed.start)) {
      return parsed as AwConfig;
    }
  } catch {
    // not valid YAML
  }
  return null;
}

// ── Diff helpers ──────────────────────────────────────────────────────────────

/** Find dimension names referenced in a field's expr. */
export function findRelatedDims(field: ViewField, allDims: ViewField[]): string[] {
  if (!field.expr) return [];
  const expr = field.expr.toLowerCase();
  return allDims.filter((d) => d.name && expr.includes(d.name.toLowerCase())).map((d) => d.name);
}

export function diffFields(
  oldFields: ViewField[],
  newFields: ViewField[],
  kind: "dim" | "measure"
): FieldDiff[] {
  const oldMap = new Map(oldFields.map((f) => [f.name, f]));
  const newMap = new Map(newFields.map((f) => [f.name, f]));
  const result: FieldDiff[] = [];

  for (const f of newFields) {
    const old = oldMap.get(f.name);
    if (!old) {
      result.push({ name: f.name, status: "added", kind, field: f });
    } else {
      const changes: string[] = [];
      if (old.type !== f.type) changes.push(`type: ${old.type ?? "–"} → ${f.type ?? "–"}`);
      if (old.description !== f.description) changes.push("description updated");
      if (old.expr !== f.expr) changes.push(`expr: ${old.expr ?? "–"} → ${f.expr ?? "–"}`);
      result.push({
        name: f.name,
        status: changes.length > 0 ? "modified" : "unchanged",
        kind,
        field: f,
        oldField: old,
        changes: changes.length > 0 ? changes : undefined
      });
    }
  }

  for (const f of oldFields) {
    if (!newMap.has(f.name)) {
      result.push({ name: f.name, status: "removed", kind, field: f });
    }
  }

  return result;
}

export function automationKind(filePath: string): string {
  if (filePath.endsWith(".procedure.yml") || filePath.endsWith(".procedure.yaml"))
    return "Automation";
  if (filePath.endsWith(".automation.yml") || filePath.endsWith(".automation.yaml"))
    return "Automation";
  return "Automation";
}

export function diffAutomationTasks(
  oldWf: AutomationConfig | null,
  newWf: AutomationConfig
): AppItemDiff[] {
  const result: AppItemDiff[] = [];
  const oldTasks = oldWf?.tasks ?? [];
  const newTasks = newWf.tasks ?? [];
  const oldTaskMap = new Map(oldTasks.map((t) => [t.name, t]));
  const newTaskMap = new Map(newTasks.map((t) => [t.name, t]));

  for (const t of newTasks) {
    const old = oldTaskMap.get(t.name);
    const label =
      t.type === "execute_sql" ? "SQL Task" : t.type === "formatter" ? "Formatter" : "Task";
    if (!old) {
      result.push({
        key: `task:${t.name}`,
        status: "added",
        kind: "task",
        label,
        title: t.name,
        subtitle: t.database ? `db: ${t.database}` : undefined
      });
    } else {
      const changes: string[] = [];
      if (old.type !== t.type) changes.push(`type: ${old.type} → ${t.type}`);
      if (old.database !== t.database)
        changes.push(`db: ${old.database ?? "–"} → ${t.database ?? "–"}`);
      if ((old.sql_query ?? old.sql_file) !== (t.sql_query ?? t.sql_file))
        changes.push("query updated");
      if (old.template !== t.template) changes.push("template updated");
      if (changes.length === 0 && JSON.stringify(old) !== JSON.stringify(t))
        changes.push("updated");
      result.push({
        key: `task:${t.name}`,
        status: changes.length > 0 ? "modified" : "unchanged",
        kind: "task",
        label,
        title: t.name,
        subtitle: t.database ? `db: ${t.database}` : undefined,
        changes: changes.length > 0 ? changes : undefined
      });
    }
  }
  for (const t of oldTasks) {
    if (!newTaskMap.has(t.name))
      result.push({
        key: `task:${t.name}`,
        status: "removed",
        kind: "task",
        label: "Task",
        title: t.name
      });
  }
  return result;
}

export function diffTopicViews(oldTopic: TopicConfig | null, newTopic: TopicConfig): AppItemDiff[] {
  const oldViews = new Set(oldTopic?.views ?? []);
  const newViews = newTopic.views ?? [];
  const result: AppItemDiff[] = newViews.map((v) => ({
    key: `view:${v}`,
    status: oldViews.has(v) ? ("unchanged" as const) : ("added" as const),
    kind: "view" as const,
    label: "View",
    title: v
  }));
  for (const v of Array.from(oldViews)) {
    if (!newViews.includes(v))
      result.push({ key: `view:${v}`, status: "removed", kind: "view", label: "View", title: v });
  }
  return result;
}

export function tryParseTest(content: string): TestFileConfig | null {
  try {
    const parsed = parseYaml(content);
    if (parsed && typeof parsed === "object" && Array.isArray(parsed.cases)) {
      return parsed as TestFileConfig;
    }
  } catch {
    // not valid YAML
  }
  return null;
}

export function diffTestCases(
  oldTest: TestFileConfig | null,
  newTest: TestFileConfig
): AppItemDiff[] {
  const result: AppItemDiff[] = [];
  const oldCases = oldTest?.cases ?? [];
  const newCases = newTest.cases ?? [];
  const caseKey = (c: TestCase, i: number) => c.name ?? `case:${i}`;
  const oldMap = new Map(oldCases.map((c, i) => [caseKey(c, i), c]));
  const newKeys = new Set(newCases.map((c, i) => caseKey(c, i)));

  newCases.forEach((c, i) => {
    const key = caseKey(c, i);
    const old = oldMap.get(key);
    const title = c.name ?? c.prompt.slice(0, 40);
    const subtitleParts = [
      c.tags && c.tags.length > 0 ? c.tags.join(", ") : undefined,
      c.tool ? `tool: ${c.tool}` : undefined
    ].filter(Boolean);
    const subtitle = subtitleParts.length > 0 ? subtitleParts.join(" · ") : undefined;
    if (!old) {
      result.push({
        key: `case:${key}`,
        status: "added",
        kind: "test_case",
        label: "Case",
        title,
        subtitle
      });
    } else {
      const changes: string[] = [];
      if (old.prompt !== c.prompt) changes.push("prompt updated");
      if (old.expected !== c.expected) changes.push("expected updated");
      if (old.tool !== c.tool) changes.push(`tool: ${old.tool ?? "–"} → ${c.tool ?? "–"}`);
      result.push({
        key: `case:${key}`,
        status: changes.length > 0 ? "modified" : "unchanged",
        kind: "test_case",
        label: "Case",
        title,
        subtitle,
        changes: changes.length > 0 ? changes : undefined
      });
    }
  });

  oldCases.forEach((c, i) => {
    const key = caseKey(c, i);
    if (!newKeys.has(key))
      result.push({
        key: `case:${key}`,
        status: "removed",
        kind: "test_case",
        label: "Case",
        title: c.name ?? c.prompt.slice(0, 40)
      });
  });

  return result;
}

export function diffAwTransitions(oldAw: AwConfig | null, newAw: AwConfig): AppItemDiff[] {
  const transKey = (t: AwTransition) => (t.database ? `${t.type}:${t.database}` : t.type);
  const oldTrans = oldAw?.transitions ?? [];
  const newTrans = newAw.transitions ?? [];
  const oldTransMap = new Map(oldTrans.map((t) => [transKey(t), t]));
  const newTransKeys = new Set(newTrans.map(transKey));

  const result: AppItemDiff[] = newTrans.map((t) => ({
    key: `trans:${transKey(t)}`,
    status: oldTransMap.has(transKey(t)) ? ("unchanged" as const) : ("added" as const),
    kind: "transition" as const,
    label: "Transition",
    title: t.type,
    subtitle: t.database ? `db: ${t.database}` : undefined
  }));
  for (const t of oldTrans) {
    if (!newTransKeys.has(transKey(t)))
      result.push({
        key: `trans:${transKey(t)}`,
        status: "removed",
        kind: "transition",
        label: "Transition",
        title: t.type
      });
  }
  return result;
}
