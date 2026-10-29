// `@oxy-hq/sdk/manifest` — the `oxy-app.json` contract, as a value.
//
// Pure TypeScript: no React, no DOM, nothing past ES2020. Its own entry so a
// Node tool (`oxyc`, the Vite plugin) can take the schema, the name rules and
// the validator without the browser half of the SDK — the two packages that
// used to carry byte copies of `notification-catalog.ts` now import it here.

export {
  FUNCTION_NAME_PATTERN,
  FUNCTION_NAME_RULE,
  isValidFunctionName,
  isValidSlug,
  SLUG_MAX_LEN,
  SLUG_PATTERN,
  SLUG_RULE
} from "./custom-app/manifest-rules";
export {
  type ManifestSchemaDocument,
  type ManifestSchemaNode,
  OXY_APP_SCHEMA,
  OXY_APP_SCHEMA_FILE,
  OXY_APP_SCHEMA_URL
} from "./custom-app/manifest-schema";
export type {
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
  OxyAppWebhookManifest
} from "./custom-app/manifest-types";

export {
  isV1Manifest,
  type ManifestIssue,
  V1_MANIFEST_MESSAGE,
  validateOxyAppManifest
} from "./custom-app/manifest-validate";

export {
  type CatalogFinding,
  checkNotifications,
  humanizeNotificationName,
  isValidCategoryPath,
  isValidNotifyKind,
  type NotificationLevel,
  type OxyAppNotificationAction,
  type OxyAppNotificationCategory,
  type OxyAppNotificationKind,
  type OxyAppNotificationsManifest,
  type ResolvedNotificationKind,
  resolveNotificationKind
} from "./custom-app/notification-catalog";

export {
  checkWidgets,
  MAX_WIDGET_ACTIONS,
  MAX_WIDGETS,
  type OxyAppWidget,
  type OxyAppWidgetAction,
  type OxyAppWidgetSize,
  type WidgetActionResult,
  type WidgetButton,
  type WidgetCheckId,
  type WidgetFinding,
  type WidgetLevel,
  type WidgetRow,
  type WidgetRowState,
  type WidgetView
} from "./custom-app/widgets";
