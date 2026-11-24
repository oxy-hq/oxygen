// {{APP_DISPLAY_NAME}} — scaffolded by `create-oxy-app`.
//
// A standard Oxy custom app: a Vite + React bundle that Oxy serves at
// `/customer-apps/<org>/{{APP_SLUG}}/` and that talks to your project through
// `@oxy-hq/sdk`. Two data surfaces are wired up below:
//
//   1. `useQuery`         — raw SQL against the project's warehouse
//   2. `useSemanticQuery` — measures + dimensions from a semantic topic
//
// Every piece on screen (Panel, Stat, DefaultTabs, PanelTable, LoadError,
// EmptyState, Loading, Chip) is the shared desk UI from `@oxy-hq/sdk/ui`:
// import it, never copy it, so the app moves with Oxygen's design system.
// `tests/design-system.test.ts` fails on a local copy.
//
// Auth rides the session cookie: Oxy only serves this bundle after a
// membership check, so `/api/*` calls from here are already authenticated —
// there is no token to manage in the frontend.
//
// To make it yours: swap `STARTER_SQL` for a real query, and point `TOPIC` at
// one of your project's `.topic.yml` topics.

import { useQuery, useSemanticQuery } from "@oxy-hq/sdk";
import {
  Chip,
  DefaultTabs,
  EmptyState,
  LoadError,
  Loading,
  Panel,
  type PanelColumn,
  PanelTable,
  Stat,
  StatGrid,
  TabPanel
} from "@oxy-hq/sdk/ui";
import { useState } from "react";

// ── Replace these with your own ─────────────────────────────────────────────

// A literal SELECT needs no tables, so a freshly scaffolded app renders real
// results on first run with nothing set up. Point it at your warehouse when
// you're ready, e.g.:
//
//   SELECT status AS dataset, COUNT(*) AS records FROM orders GROUP BY status
const STARTER_SQL = `
  SELECT 'orders' AS dataset, 1284 AS records
  UNION ALL SELECT 'customers', 317
  UNION ALL SELECT 'shipments', 902
`;

// A topic from your project's semantic model (a `.topic.yml` file), plus one
// dimension and one measure it exposes. Member paths are `topic.member`. The
// tab below explains itself until these point at something real.
const TOPIC = "your_topic";
const DIMENSION = "your_dimension";
const MEASURE = "your_measure";

interface StarterRow {
  dataset: string;
  records: number;
}

type Tab = "warehouse" | "semantic";

const fmtInt = (n: number) => n.toLocaleString();

export function App() {
  const starter = useQuery<StarterRow>({ sql: STARTER_SQL });
  const [tab, setTab] = useState<Tab>("warehouse");

  const totalRecords = starter.rows.reduce((sum, r) => sum + Number(r.records ?? 0), 0);
  const status = starter.error ? "error" : starter.loading ? "connecting" : "connected";
  const figure = (n: number) => (starter.loading ? "…" : fmtInt(n));

  return (
    <main className='flex h-full w-full flex-col gap-3 overflow-auto p-3'>
      <p className='text-muted-foreground text-sm'>
        A standard Oxy custom app. Edit <code>src/App.tsx</code> to make it yours.
      </p>

      <StatGrid>
        <Stat boxed label='Rows returned' value={figure(starter.rows.length)} />
        <Stat boxed label='Total records' value={figure(totalRecords)} />
        <Stat boxed label='Project connection' value={status} />
      </StatGrid>

      <DefaultTabs
        group='starter'
        label='Data surface'
        value={tab}
        onChange={setTab}
        hasPanel
        options={[
          { value: "warehouse", label: "Warehouse query" },
          { value: "semantic", label: "Semantic model" }
        ]}
      />
      <TabPanel group='starter' value={tab}>
        {tab === "warehouse" ? <StarterQueryPanel query={starter} /> : <SemanticPanel />}
      </TabPanel>

      <Panel title='Next steps'>
        <p className='text-muted-foreground text-sm'>
          Replace <code>STARTER_SQL</code> with a query against your warehouse, point{" "}
          <code>TOPIC</code> at one of your semantic topics, then ship it with{" "}
          <code>oxyc publish</code>. The look is Oxygen's: the pieces come from{" "}
          <code>@oxy-hq/sdk/ui</code> and the colours from <code>@oxy-hq/sdk/theme.css</code>.
        </p>
      </Panel>
    </main>
  );
}

// ── useQuery · raw SQL ──────────────────────────────────────────────────────

function StarterQueryPanel({ query }: { query: ReturnType<typeof useQuery<StarterRow>> }) {
  if (query.loading) return <Loading label='Running the starter query…' rows={3} cols={2} />;
  if (query.error) {
    return (
      <LoadError title='The starter query failed' onRetry={query.refetch}>
        {query.error.message}
      </LoadError>
    );
  }
  return (
    <Panel title='Warehouse query · useQuery' right='raw SQL'>
      <PanelTable
        label='Starter query'
        rows={query.rows}
        rowKey={(row) => row.dataset}
        columns={[
          { header: "Dataset", cell: (row) => row.dataset },
          { header: "Records", align: "right", cell: (row) => fmtInt(Number(row.records)) }
        ]}
        footnote={
          <>
            These rows are literals so the scaffold renders with no tables set up. Swap{" "}
            <code>STARTER_SQL</code> in <code>src/App.tsx</code> for a real query.
          </>
        }
      />
    </Panel>
  );
}

// ── useSemanticQuery · the semantic model ───────────────────────────────────
//
// The same shape as useQuery, but the measures come from your `.view.yml` /
// `.topic.yml` files instead of SQL in this file. When the data team refactors
// the SQL behind a measure, this panel follows with no edit here.

function SemanticPanel() {
  const semantic = useSemanticQuery<Record<string, unknown>>({
    topic: TOPIC,
    dimensions: [`${TOPIC}.${DIMENSION}`],
    measures: [`${TOPIC}.${MEASURE}`],
    limit: 5
  });

  if (semantic.loading) return <Loading label='Reading the semantic model…' rows={3} cols={2} />;

  // Until TOPIC points at a real topic this errors — that is expected on a
  // fresh scaffold, so present it as the next step rather than a failure.
  if (semantic.error || semantic.rows.length === 0) {
    return (
      <EmptyState
        title='The semantic model is not set up yet'
        actions={<Chip tone='warning'>Not set up</Chip>}
      >
        Point <code>TOPIC</code>, <code>DIMENSION</code> and <code>MEASURE</code> in{" "}
        <code>src/App.tsx</code> at a topic your project defines (a <code>.topic.yml</code> file)
        and this tab fills in. Member paths are <code>topic.member</code>.
      </EmptyState>
    );
  }

  const columns = semantic.columns.length > 0 ? semantic.columns : Object.keys(semantic.rows[0]);
  return (
    <Panel title='Semantic model · useSemanticQuery' right={`topic: ${TOPIC}`}>
      <PanelTable
        label='Semantic query'
        rowHeader={false}
        rows={semantic.rows}
        // Semantic rows have no natural id; the index is stable for one result set.
        rowKey={(_row, i) => String(i)}
        columns={columns.map(
          (c): PanelColumn<Record<string, unknown>> => ({ header: c, cell: (row) => String(row[c]) })
        )}
      />
    </Panel>
  );
}
