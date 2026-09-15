import Editor from "@monaco-editor/react";
import { BadgeCheck, Zap } from "lucide-react";
import SqlArtifactPanel from "@/components/ArtifactPanel/ArtifactsContent/sql";
import SqlResultsTable from "@/components/sql/SqlResultsTable";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Panel, PanelContent, PanelHeader } from "@/components/ui/panel";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import type { ArtifactItem, AutomationItem, SqlItem } from "@/hooks/analyticsSteps";
import type { AnalyticsDisplayBlock, SseEvent } from "@/hooks/useAnalyticsRun";
import { extractDisplayBlockForSeq } from "@/hooks/useAnalyticsRun";
import useTheme from "@/stores/useTheme";
import { VERIFIED_SQL_FILE_TOOLTIP, VERIFIED_TOOLTIP } from "../constants";
import {
  AnalyzeDbtProjectView,
  AskUserView,
  AutomationStepView,
  ChartSection,
  CleanDbtProjectView,
  ColumnRangeView,
  ColumnValuesView,
  CompileDbtModelView,
  CompileSemanticQueryView,
  CountRowsView,
  DebugDbtProjectView,
  DocsGenerateDbtView,
  ExplainMetricView,
  FindOpportunitiesView,
  FormatDbtSqlView,
  GetDbtColumnLineageView,
  GetDbtLineageView,
  GetJoinPathView,
  GetMetricDefinitionView,
  InitDbtProjectView,
  ListDbtNodesView,
  MetricSensitivityView,
  ParseDbtProjectView,
  PredictImpactView,
  RawArtifactView,
  RenderChartView,
  ResolveSchemaView,
  RunDbtModelsView,
  SampleColumnView,
  SearchAutomationsView,
  SearchCatalogView,
  SeedDbtProjectView,
  TestDbtModelsView
} from "./AnalyticsArtifactViews";
import {
  sqlArtifactFromExecutePreview,
  sqlArtifactFromExecuteSql,
  sqlArtifactFromPreviewData,
  sqlArtifactFromSemanticQuery,
  sqlArtifactFromSqlItem
} from "./analyticsArtifactHelpers";
import {
  ExecuteSqlView,
  FileChangeToolView,
  LookupSchemaView,
  ManageDirectoryView,
  ReadFileView,
  RunTestsView,
  SearchFilesView,
  SearchTextView,
  SemanticQueryView,
  ValidateProjectView,
  VerifiedSemanticQueryView
} from "./BuilderArtifactViews";
import ListDbtProjectsView from "./ListDbtProjectsView";
import SubrunDagPanel from "./SubrunDagPanel";

interface Props {
  item: ArtifactItem | SqlItem | AutomationItem;
  displayBlocks?: AnalyticsDisplayBlock[];
  runEvents?: SseEvent[];
  isRunning?: boolean;
  onClose: () => void;
}

const AnalyticsArtifactSidebar = ({
  item,
  displayBlocks = [],
  runEvents = [],
  isRunning = false,
  onClose
}: Props) => {
  const theme = useTheme((s) => s.theme);
  const monacoTheme = theme === "dark" ? "vs-dark" : "vs";
  // ── kind === "automation" → full DAG panel ─────────────────────────────────
  if (item.kind === "automation") {
    return (
      <SubrunDagPanel
        automationName={item.automationName}
        steps={item.steps}
        events={runEvents}
        isRunning={isRunning}
        onClose={onClose}
      />
    );
  }

  // ── kind === "sql" (query_executed domain event) ──────────────────────────
  if (item.kind === "sql") {
    const isSemantic = item.source === "semantic";
    const isVerifiedSqlFile = item.source === "verified_sql";
    const verified = isSemantic || isVerifiedSqlFile;
    const label = isSemantic
      ? "Semantic Query"
      : isVerifiedSqlFile
        ? "Verified Query"
        : "SQL Query";
    const verifiedTooltip = isVerifiedSqlFile ? VERIFIED_SQL_FILE_TOOLTIP : VERIFIED_TOOLTIP;
    const title = (
      <div className='flex min-w-0 items-center gap-1.5'>
        <h3 className='truncate font-semibold text-sm'>{label}</h3>
        {verified && (
          <Tooltip>
            <TooltipTrigger asChild>
              <BadgeCheck className='h-4 w-4 shrink-0 text-oxy-blue-600 dark:text-oxy-blue-500' />
            </TooltipTrigger>
            <TooltipContent side='bottom'>{verifiedTooltip}</TooltipContent>
          </Tooltip>
        )}
        {item.is_preagg && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Zap className='h-4 w-4 shrink-0 text-primary' />
            </TooltipTrigger>
            <TooltipContent side='bottom'>Served from pre-aggregation cache</TooltipContent>
          </Tooltip>
        )}
      </div>
    );
    const subtitle =
      item.rowCount !== undefined ? `${item.rowCount} rows · ${item.durationMs ?? 0}ms` : undefined;

    if (verified && item.semanticQuery) {
      const sqlLineCount = item.sql.split("\n").length;
      const sqlHeight = Math.min(Math.max(sqlLineCount * 18 + 24, 120), 320);
      return (
        <Panel>
          <PanelHeader title={title} subtitle={subtitle} onClose={onClose} />
          <PanelContent scrollable={true} padding={false} className='flex min-h-0 flex-col'>
            <VerifiedSemanticQueryView query={item.semanticQuery} database={item.database} />
            <div className='border-t'>
              <div className='px-4 pt-3 pb-1.5 font-medium text-muted-foreground text-xs uppercase tracking-wide'>
                Compiled SQL
              </div>
              <div style={{ height: sqlHeight }}>
                <Editor
                  height='100%'
                  width='100%'
                  theme={monacoTheme}
                  defaultValue={item.sql}
                  language='sql'
                  value={item.sql}
                  loading={<Spinner />}
                  options={{
                    readOnly: true,
                    scrollBeyondLastLine: false,
                    automaticLayout: true,
                    minimap: { enabled: false }
                  }}
                />
              </div>
            </div>
            {item.error && (
              <ErrorAlert className='mx-3 my-2 max-h-32 overflow-y-auto' message={item.error} />
            )}
            {!item.error && item.result && (
              <div className='border-t'>
                <div className='px-4 pt-3 pb-1.5 font-medium text-muted-foreground text-xs uppercase tracking-wide'>
                  Results
                </div>
                <SqlResultsTable result={item.result} />
              </div>
            )}
          </PanelContent>
        </Panel>
      );
    }

    return (
      <Panel>
        <PanelHeader title={title} subtitle={subtitle} onClose={onClose} />
        <PanelContent scrollable={false} padding={false} className='flex min-h-0 flex-col'>
          <div className='min-h-0 flex-1'>
            <SqlArtifactPanel artifact={sqlArtifactFromSqlItem(item)} />
          </div>
        </PanelContent>
      </Panel>
    );
  }

  // ── execute_preview → SQL panel ───────────────────────────────────────────
  if (item.toolName === "execute_preview") {
    const sqlArtifact = sqlArtifactFromExecutePreview(item);
    return (
      <Panel>
        <PanelHeader
          title='Preview Query'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          {sqlArtifact ? (
            <SqlArtifactPanel artifact={sqlArtifact} />
          ) : (
            <RawArtifactView item={item} />
          )}
        </PanelContent>
      </Panel>
    );
  }

  // ── render_chart → config + rendered chart ────────────────────────────────
  if (item.toolName === "render_chart") {
    const block =
      item.seq != null
        ? extractDisplayBlockForSeq(runEvents, item.seq)
        : (displayBlocks[0] ?? null);
    return (
      <Panel>
        <PanelHeader
          title='Render Chart'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='flex min-h-0 flex-col'>
          <div className='min-h-0 flex-1 overflow-auto'>
            <RenderChartView item={item} />
          </div>
          <ChartSection displayBlocks={block ? [block] : []} />
        </PanelContent>
      </Panel>
    );
  }

  // ── ask_user → question + user response ──────────────────────────────────
  if (item.toolName === "ask_user") {
    return (
      <Panel>
        <PanelHeader
          title='Ask User'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <AskUserView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── search_catalog → structured catalog view ──────────────────────────────
  if (item.toolName === "search_catalog") {
    return (
      <Panel>
        <PanelHeader
          title='Catalog Search'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <SearchCatalogView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "search_files") {
    return (
      <Panel>
        <PanelHeader
          title='Search Files'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <SearchFilesView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "search_text") {
    return (
      <Panel>
        <PanelHeader
          title='Search Text'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <SearchTextView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "preview_data") {
    const sqlArtifact = sqlArtifactFromPreviewData(item);
    return (
      <Panel>
        <PanelHeader
          title='Preview Table'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          {sqlArtifact ? (
            <SqlArtifactPanel artifact={sqlArtifact} />
          ) : (
            <RawArtifactView item={item} />
          )}
        </PanelContent>
      </Panel>
    );
  }

  // ── metric-tree tools ─────────────────────────────────────────────────────
  if (item.toolName === "explain_metric") {
    return (
      <Panel>
        <PanelHeader
          title='Explain Metric'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <ExplainMetricView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "find_opportunities") {
    return (
      <Panel>
        <PanelHeader
          title='Find Opportunities'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <FindOpportunitiesView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "metric_sensitivity") {
    return (
      <Panel>
        <PanelHeader
          title='Metric Sensitivity'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <MetricSensitivityView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "predict_impact") {
    return (
      <Panel>
        <PanelHeader
          title='Predict Impact'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <PredictImpactView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── search_automations → automation list ──────────────────────────────────
  // Match the legacy tool name too so runs persisted before the rename render.
  if (item.toolName === "search_automations" || item.toolName === "search_procedures") {
    return (
      <Panel>
        <PanelHeader
          title='Automation Search'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <SearchAutomationsView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "read_file") {
    return (
      <Panel>
        <PanelHeader
          title='Read File'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <ReadFileView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "execute_sql") {
    const sqlArtifact = sqlArtifactFromExecuteSql(item);
    return (
      <Panel>
        <PanelHeader
          title='Execute SQL'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='flex min-h-0 flex-col'>
          <div className='shrink-0'>
            <ExecuteSqlView item={item} />
          </div>
          <div className='min-h-0 flex-1'>
            {sqlArtifact ? (
              <SqlArtifactPanel artifact={sqlArtifact} />
            ) : (
              <RawArtifactView item={item} />
            )}
          </div>
        </PanelContent>
      </Panel>
    );
  }

  if (
    item.toolName === "file_change" ||
    item.toolName === "write_file" ||
    item.toolName === "edit_file" ||
    item.toolName === "delete_file"
  ) {
    const title =
      item.toolName === "write_file"
        ? "Write File"
        : item.toolName === "edit_file"
          ? "Edit File"
          : item.toolName === "delete_file"
            ? "Delete File"
            : "File Change";
    return (
      <Panel>
        <PanelHeader
          title={title}
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <FileChangeToolView item={item} runEvents={runEvents} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "lookup_schema") {
    return (
      <Panel>
        <PanelHeader
          title='Lookup Schema'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <LookupSchemaView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "semantic_query") {
    const sqlArtifact = sqlArtifactFromSemanticQuery(item);
    return (
      <Panel>
        <PanelHeader
          title='Semantic Query'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='flex min-h-0 flex-col'>
          <div className='shrink-0'>
            <SemanticQueryView item={item} />
          </div>
          {sqlArtifact ? (
            <div className='min-h-0 flex-1'>
              <SqlArtifactPanel artifact={sqlArtifact} />
            </div>
          ) : isRunning ? (
            <div className='flex items-center justify-center p-4'>
              <Spinner />
            </div>
          ) : null}
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "validate_project") {
    return (
      <Panel>
        <PanelHeader
          title='Validate Project'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <ValidateProjectView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "run_tests") {
    return (
      <Panel>
        <PanelHeader
          title='Run Tests'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <RunTestsView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── get_metric_definition → metric definition ─────────────────────────────
  if (item.toolName === "get_metric_definition") {
    return (
      <Panel>
        <PanelHeader
          title='Metric Definition'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <GetMetricDefinitionView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── get_join_path → join path ─────────────────────────────────────────────
  if (item.toolName === "get_join_path") {
    return (
      <Panel>
        <PanelHeader
          title='Join Path'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <GetJoinPathView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── sample_column / sample_columns → column explorer ─────────────────────
  if (item.toolName === "sample_column" || item.toolName === "sample_columns") {
    return (
      <Panel>
        <PanelHeader
          title='Column Sample'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <SampleColumnView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "get_column_values") {
    return (
      <Panel>
        <PanelHeader
          title='Column Values'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <ColumnValuesView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "get_column_range") {
    return (
      <Panel>
        <PanelHeader
          title='Column Range'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <ColumnRangeView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  if (item.toolName === "count_rows") {
    return (
      <Panel>
        <PanelHeader
          title='Count Rows'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <CountRowsView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── propose_semantic_query → semantic shortcut query ─────────────────────
  if (item.toolName === "propose_semantic_query") {
    return (
      <Panel>
        <PanelHeader
          title='Semantic Query'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <CompileSemanticQueryView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── compile_semantic_query → airlayer compile result ─────────────────────
  if (item.toolName === "compile_semantic_query") {
    return (
      <Panel>
        <PanelHeader
          title='Compile Semantic Query'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <CompileSemanticQueryView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── resolve_schema → schema tables ───────────────────────────────────────
  if (item.toolName === "resolve_schema") {
    return (
      <Panel>
        <PanelHeader
          title='Schema'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <ResolveSchemaView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── test_dbt_models ───────────────────────────────────────────────────────
  if (item.toolName === "test_dbt_models") {
    return (
      <Panel>
        <PanelHeader
          title='Test dbt Models'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <TestDbtModelsView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── list_dbt_projects ─────────────────────────────────────────────────────
  if (item.toolName === "list_dbt_projects") {
    return (
      <Panel>
        <PanelHeader
          title='List dbt Projects'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <ListDbtProjectsView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── list_dbt_nodes ────────────────────────────────────────────────────────
  if (item.toolName === "list_dbt_nodes") {
    return (
      <Panel>
        <PanelHeader
          title='List dbt Nodes'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <ListDbtNodesView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── get_dbt_lineage ───────────────────────────────────────────────────────
  if (item.toolName === "get_dbt_lineage") {
    return (
      <Panel>
        <PanelHeader
          title='dbt Lineage'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <GetDbtLineageView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── compile_dbt_model ─────────────────────────────────────────────────────
  if (item.toolName === "compile_dbt_model") {
    return (
      <Panel>
        <PanelHeader
          title='Compile dbt Model'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <CompileDbtModelView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── run_dbt_models ────────────────────────────────────────────────────────
  if (item.toolName === "run_dbt_models") {
    return (
      <Panel>
        <PanelHeader
          title='Run dbt Models'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <RunDbtModelsView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── analyze_dbt_project ───────────────────────────────────────────────────
  if (item.toolName === "analyze_dbt_project") {
    return (
      <Panel>
        <PanelHeader
          title='Analyze dbt Project'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <AnalyzeDbtProjectView key={item.id ?? item.toolName} item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── get_dbt_column_lineage ────────────────────────────────────────────────
  if (item.toolName === "get_dbt_column_lineage") {
    return (
      <Panel>
        <PanelHeader
          title='Column Lineage'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <GetDbtColumnLineageView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── parse_dbt_project ─────────────────────────────────────────────────────
  if (item.toolName === "parse_dbt_project") {
    return (
      <Panel>
        <PanelHeader
          title='Parse dbt Project'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <ParseDbtProjectView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── seed_dbt_project ──────────────────────────────────────────────────────
  if (item.toolName === "seed_dbt_project") {
    return (
      <Panel>
        <PanelHeader
          title='Seed dbt Project'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <SeedDbtProjectView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── debug_dbt_project ─────────────────────────────────────────────────────
  if (item.toolName === "debug_dbt_project") {
    return (
      <Panel>
        <PanelHeader
          title='Debug dbt Project'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <DebugDbtProjectView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── clean_dbt_project ─────────────────────────────────────────────────────
  if (item.toolName === "clean_dbt_project") {
    return (
      <Panel>
        <PanelHeader
          title='Clean dbt Project'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <CleanDbtProjectView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── docs_generate_dbt ─────────────────────────────────────────────────────
  if (item.toolName === "docs_generate_dbt") {
    return (
      <Panel>
        <PanelHeader
          title='Generate dbt Docs'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <DocsGenerateDbtView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── format_dbt_sql ────────────────────────────────────────────────────────
  if (item.toolName === "format_dbt_sql") {
    return (
      <Panel>
        <PanelHeader
          title='Format dbt SQL'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <FormatDbtSqlView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── manage_directory → directory operation view ───────────────────────────
  if (item.toolName === "manage_directory") {
    return (
      <Panel>
        <PanelHeader
          title='Manage Directory'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <ManageDirectoryView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── init_dbt_project ──────────────────────────────────────────────────────
  if (item.toolName === "init_dbt_project") {
    return (
      <Panel>
        <PanelHeader
          title='Init dbt Project'
          subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <InitDbtProjectView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── Automation step → status view ─────────────────────────────────────────
  // toolInput is the literal string "Running…" for automation steps, never JSON.
  if (item.toolInput === "Running\u2026") {
    return (
      <Panel>
        <PanelHeader
          title={item.toolName}
          subtitle={
            item.isStreaming ? "Running" : item.toolOutput === "Completed" ? "Completed" : "Failed"
          }
          onClose={onClose}
        />
        <PanelContent scrollable={false} padding={false} className='min-h-0'>
          <AutomationStepView item={item} />
        </PanelContent>
      </Panel>
    );
  }

  // ── Generic fallback ──────────────────────────────────────────────────────
  return (
    <Panel>
      <PanelHeader
        title={item.toolName}
        subtitle={item.durationMs !== undefined ? `${item.durationMs}ms` : undefined}
        onClose={onClose}
      />
      <PanelContent scrollable={false} padding={false} className='min-h-0'>
        <RawArtifactView item={item} />
      </PanelContent>
    </Panel>
  );
};

export default AnalyticsArtifactSidebar;
