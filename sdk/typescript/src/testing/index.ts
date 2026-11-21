// `@oxy-hq/sdk/testing` — a typed test context for Oxy Functions, and a fake
// inbox for an app's UI tests.
//
// One factory, `createTestContext(manifest, options)`, returning an
// `OxyFunctionContext` that refuses where the host refuses, in the host's own
// words, answers reads from the shape zoo, and records every host op the
// function makes, and is held to the host by a drift test on the host side.
//
// NOT for use inside a function: the isolate never loads it, and `oxyc
// publish` bundles only what a function imports. Pure TypeScript, no React —
// its own entry, so a value import in a test never pulls in the root entry.
//
// `createTestInbox()` answers `/api/notifications` from items a test holds,
// so `<NotificationsBell />` renders under `<OxyAppProvider fetcher={…}>` with
// no server (`inbox.ts`). It is not part of the host contract.

// The notification catalogue's publish checks and the inbox's read of it, for
// an app test that asserts its own `oxy-app.json` (`createTestContext` already
// refuses a catalogue publish would).
export {
  type CatalogFinding,
  checkNotifications,
  type ResolvedNotificationKind,
  resolveNotificationKind
} from "../custom-app/notification-catalog";
// Home widgets' publish checks, for the same kind of test.
export { checkWidgets, type WidgetFinding } from "../custom-app/widgets";
export {
  createTestApps,
  type SentAppCall,
  type TestAppHandler,
  type TestApps
} from "./apps";
export {
  createTestContext,
  type HostCall,
  type HostOpImpl,
  type TestContext,
  type TestContextOptions,
  type TestDatabase,
  type TestFunctionContext,
  type TestState
} from "./context";
export {
  createTestEvents,
  type PublishedEvent,
  type TableChangeInput,
  type TestEvents
} from "./events";
export { isSafeOutbound } from "./fetch-rules";
export { type FunctionGates, type ManifestLike, readGates, writerSchema } from "./gates";
export {
  ABSENT_GLOBALS,
  DIALECTS,
  type Dialect,
  FETCH_MAX_BYTES,
  FETCH_RULES,
  GATES,
  type Gate,
  HOST_OPS,
  type HostOp,
  MAX_NOTIFY_THREAD_CHARS,
  MAX_OPEN_TRANSACTIONS,
  REFUSALS,
  RESERVED_NOTIFY_KIND_PREFIX,
  type RefusalTemplate,
  SURFACES
} from "./host-contract";
export {
  contextError,
  HOST_ERROR_NAME,
  hostError,
  isHostError,
  refuse,
  render
} from "./host-error";
export {
  createTestInbox,
  type NotifyPayload,
  type TestInbox,
  type TestInboxOptions,
  type TestInboxRequest,
  threadKey
} from "./inbox";
export { isolateAtob, isolateBtoa, runWithoutIsolateGlobals } from "./run";
// The no-copy guard for an app's src/: no component named like an @oxy-hq/sdk/ui piece, unless a
// `// oxy-ui-local: <Name> -- <why>` line keeps it (sdkUiCopies.ts).
export {
  type CheckSdkUiCopiesOptions,
  checkSdkUiCopies,
  formatSdkUiCopies,
  SDK_UI_PIECES,
  type SdkUiCopyFinding,
  type SdkUiCopyKind
} from "./sdkUiCopies";
export { ZOO_SHA256 } from "./shape-zoo-sha256";
export {
  type FetchAnswer,
  FetchStore,
  type ReadResult,
  type StoredObject,
  TableStore,
  TypedTable
} from "./stores";
// The theme guard for an app's own CSS and index.html <style>: no SDK token restated, no literal
// colour in @theme, no bare .dark/.light, no own color-scheme or mode block (themeGuard.ts).
export {
  type CheckThemeCssOptions,
  checkThemeCss,
  formatThemeFindings,
  SDK_THEME_NAMES,
  type ThemeCssSource,
  type ThemeFinding,
  type ThemeGuardRule
} from "./themeGuard";
export {
  casesFor,
  expectation,
  knownTypes,
  type RowSource,
  renderValue,
  type ShapeZoo,
  ZOO,
  type ZooCase,
  type ZooEngine,
  type ZooPlane,
  ZooRefusal,
  zooColumn,
  zooTableName
} from "./zoo";
