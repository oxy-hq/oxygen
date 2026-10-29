// Customer-app bundle helpers — manifest loading + query execution.
//
// Customer apps are static bundles served by oxy at
// `app.oxygen-hq.com/customer-apps/<org_slug>/<app_slug>/`. They commit a
// `public/oxy-app.json` declaring their identity (slug, orgSlug,
// projectId), then call `useQuery` inside React components to run SQL
// queries against the linked project.
//
// Minimal usage:
//
// ```ts
// import { OxyAppProvider, useQuery } from "@oxy-hq/sdk";
//
// function App() {
//   return <OxyAppProvider><Dashboard /></OxyAppProvider>;
// }
// function Dashboard() {
//   const { rows, columns, loading, error } = useQuery({ sql: "SELECT * FROM orders LIMIT 10" });
//   ...
// }
// ```
//
// See `crates/app/src/server/api/custom_apps.rs` for the server-side
// query contract.

export { base64ToBytes, bytesToBase64 } from "./base64";
export type { CustomAppDebugSnapshot } from "./debug";
export { getCustomAppDebug } from "./debug";
export type { CustomAppErrorReport } from "./errors";
export { apiErrorFromResponse, interpretCustomAppError, OxyApiError } from "./errors";
export type {
  EmailAttachment,
  EmailSendInput,
  EmailSendResult,
  NotifyInput,
  NotifyResult,
  NotifyTarget,
  OxyAirhouseApi,
  OxyAirwayApi,
  OxyCryptoApi,
  OxyEmailApi,
  OxyFetchResult,
  OxyFunctionContext,
  OxyFunctionHandler,
  OxyFunctionRequest,
  OxyFunctionRow,
  OxyFunctionUser,
  OxyHmacInput,
  OxyIdentityKind,
  OxyOltpApi,
  OxyOrgAssignment,
  OxyOrgPlace,
  OxyOrgTeam,
  OxyPlatformApi,
  OxyPlatformFacts,
  OxyPlatformOrg,
  OxyPlatformWeek,
  OxyReach,
  OxySecretsApi,
  OxySemanticApi,
  OxyStorageApi,
  OxyTransaction,
  OxyVerifyHmacInput,
  OxyWarehouseApi,
  StorageDownloadUrl,
  StorageListPage,
  StorageObject,
  StoragePutOptions,
  StoragePutResult,
  StorageUploadUrl,
  StorageUploadUrlInput
} from "./function-context";
// The SSE reader's types. `FunctionError` carries `status` and `body` for a
// function that answered a non-2xx, and without this line a TypeScript consumer
// gets `unknown` from a `catch` and has to hand-roll the shape — which makes
// the whole point of surfacing the status unreachable from the public API.
export type { FunctionError, FunctionLog, FunctionResult } from "./function-sse";
export type { OxyAppEnvironment, OxyInjectedAppConfig } from "./inject";
export { readAppEnvironment, readInjectedAppConfig } from "./inject";
export type { OxyAppLogger, OxyAppLogLevel } from "./logger";
export { getOxyAppLogger, setOxyAppLogger } from "./logger";
// The manifest's types. The rules, the generated JSON Schema and the validator
// are `@oxy-hq/sdk/manifest` — pure TS, for Node tools — not the root entry.
export type {
  LoadManifestOptions,
  NotificationLevel,
  OxyAppAirwayStepManifest,
  OxyAppAskManifest,
  OxyAppBuildManifest,
  OxyAppCacheManifest,
  OxyAppEmailCapability,
  OxyAppEnabledCapability,
  OxyAppEnvDeclaration,
  OxyAppEnvironmentManifest,
  OxyAppFetchManifest,
  OxyAppFunctionManifest,
  OxyAppManifest,
  OxyAppMigrationsManifest,
  OxyAppNonProductionManifest,
  OxyAppNotificationAction,
  OxyAppNotificationCategory,
  OxyAppNotificationKind,
  OxyAppNotificationsManifest,
  OxyAppNotifyCapability,
  OxyAppNotifyKindCapability,
  OxyAppOrgCapability,
  OxyAppPerformanceManifest,
  OxyAppPlatformCapability,
  OxyAppRetentionRule,
  OxyAppRetriesManifest,
  OxyAppSecretsCapability,
  OxyAppStorageCapability,
  OxyAppStorageManifest,
  OxyAppWebhookManifest,
  ResolvedCustomAppManifest
} from "./manifest";
export {
  _resetCustomAppManifestCacheForTest,
  loadCustomAppManifest
} from "./manifest";
// Metric-tree analysis hooks (drivers / what-if / RCA / opportunity sizing /
// scenario forecasting)
export type {
  MetricTreeHookResult,
  UseMetricTreeOpts,
  UsePredictOpts
} from "./metric-tree-hooks";
export {
  useBaseline,
  useDistribution,
  useExplain,
  useMetricTree,
  useOpportunity,
  usePredict,
  useProjection,
  useSensitivity,
  useTimeDimensions
} from "./metric-tree-hooks";
// The in-app inbox: the person's notifications, read from inside the app
export type {
  NotificationAction,
  NotificationApp,
  NotificationCategoryRef,
  NotificationGroup,
  NotificationItem,
  NotificationsFilter,
  NotificationsPage,
  NotificationsScope
} from "./notifications";
export { collapseThreads } from "./notifications";
export type { NotificationsBellProps } from "./notifications-bell";
export { NotificationsBell } from "./notifications-bell";
export type { UseNotificationsOptions, UseNotificationsResult } from "./notifications-hooks";
export { useNotifications } from "./notifications-hooks";
export type {
  AgentArtifact,
  AgentRunEvent,
  AgentRunState,
  AgentSqlArtifact,
  AppFetcher,
  OxyAnswerProps,
  OxyAppProviderProps,
  OxyChatProps,
  ProcedureProgress,
  ProcedureResult,
  ProcedureRunState,
  SemanticArrayOp,
  SemanticDateRangeOp,
  SemanticFilter,
  SemanticScalarOp,
  SemanticTimeDimension,
  UseAgentRunInput,
  UseAgentRunResult,
  UseFunctionResult,
  UseProcedureRunInput,
  UseProcedureRunOpts,
  UseProcedureRunResult,
  UseQueryInput,
  UseQueryOpts,
  UseQueryResult,
  UseSemanticQueryInput,
  UseSemanticQueryOpts,
  UseSemanticQueryResult
} from "./react";
// React provider + hooks
export {
  OxyAnswer,
  OxyAppProvider,
  OxyChat,
  useAgentRun,
  useFunction,
  useOxyApp,
  useProcedureRun,
  useQuery,
  useResolvedManifest,
  useSemanticQuery,
  useTrackEvent
} from "./react";
// World-model hooks (graph / instances / driver-tree)
export { readJsonSseStream } from "./sse";
export type { AppUpdateState } from "./update-hooks";
export { useAppUpdate, useBusyWhile } from "./update-hooks";
export type { OxyUpdateAgent, OxyUpdateStatus } from "./updates";
export {
  CLIENT_BUILD_HEADER,
  holdUpdates,
  LIVE_BUILD_HEADER,
  readUpdateAgent,
  withClientBuild
} from "./updates";
// Home widgets: the manifest block's types and the view a widget function
// returns (`import type { WidgetView } from "@oxy-hq/sdk"`). The checks are
// `@oxy-hq/sdk/manifest`.
export type {
  OxyAppWidget,
  OxyAppWidgetAction,
  OxyAppWidgetSize,
  WidgetActionResult,
  WidgetButton,
  WidgetLevel,
  WidgetRow,
  WidgetRowState,
  WidgetView
} from "./widgets";
export type {
  UseMeasureBreakdownResult,
  UseWorldModelGraphResult,
  UseWorldModelInstancesOpts,
  UseWorldModelInstancesResult
} from "./world-model-hooks";
export {
  useMeasureBreakdown,
  useWorldModelGraph,
  useWorldModelInstances
} from "./world-model-hooks";
// World Model node interface (the `expand` / `drill` / `explain` / `size` paradigm)
export type {
  ExpandedNode,
  ExplainOpts,
  MetricHandle,
  MetricScope,
  SizeOpts,
  WorldModelApi
} from "./world-node";
export { createWorldModel, useWorldModel, WorldModelScopeUnsupportedError } from "./world-node";
