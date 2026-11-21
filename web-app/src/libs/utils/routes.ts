/**
 * A workspace's pages under `wsBase`. `root` is its landing page: the base itself
 * where the base names the workspace, `/<org>/home` where it is the org, whose
 * own root is the org dispatcher.
 */
function workspaceRoutes(wsBase: string, root: string) {
  return {
    ROOT: root,
    HOME: `${wsBase}/home`,
    NEW: `${wsBase}/new`,

    THREADS: `${wsBase}/threads`,
    THREAD: (threadId: string) => `${wsBase}/threads/${threadId}`,

    // Canonical "Automations" routes (formerly Automations / Automations).
    AUTOMATIONS: `${wsBase}/automations`,
    AUTOMATION: (pathb64: string) => ({
      ROOT: `${wsBase}/automations/${pathb64}`
    }),

    // Back-compat aliases — same key names kept so existing call sites
    // compile unchanged, but they now resolve to the canonical
    // `/automations` paths. The legacy `/automations` URL still renders
    // (see App.tsx) for old bookmarks/deep links.
    WORKFLOWS: `${wsBase}/automations`,
    WORKFLOW: (pathb64: string) => ({
      ROOT: `${wsBase}/automations/${pathb64}`
    }),

    PIPELINE: (pathb64: string) => ({
      ROOT: `${wsBase}/pipelines/${pathb64}`,
      RUN: (runId: string) => `${wsBase}/pipelines/${pathb64}/runs/${runId}`
    }),

    IDE: {
      ROOT: `${wsBase}/ide`,
      FILES: {
        ROOT: `${wsBase}/ide/files`,
        FILE: (pathb64: string) => `${wsBase}/ide/files/${pathb64}`
      },
      DATABASE: {
        ROOT: `${wsBase}/ide/database`
      },
      COORDINATOR: {
        ROOT: `${wsBase}/ide/coordinator`,
        OVERVIEW: `${wsBase}/ide/coordinator/overview`,
        JOBS: `${wsBase}/ide/coordinator/jobs`,
        JOB_DETAIL: (scheduleId: string) => `${wsBase}/ide/coordinator/jobs/${scheduleId}`,
        RUNS: `${wsBase}/ide/coordinator/runs`,
        RUN_DETAIL: (runId: string) => `${wsBase}/ide/coordinator/runs/${runId}`,
        RECOVERY: `${wsBase}/ide/coordinator/recovery`,
        QUEUE: `${wsBase}/ide/coordinator/queue`
      },
      OBSERVABILITY: {
        ROOT: `${wsBase}/ide/observability`,
        TRACES: `${wsBase}/ide/observability/traces`,
        TRACE: (traceId: string) => `${wsBase}/ide/observability/traces/${traceId}`,
        METRICS: `${wsBase}/ide/observability/metrics`,
        METRIC: (metricName: string) =>
          `${wsBase}/ide/observability/metrics/${encodeURIComponent(metricName)}`,
        EXECUTION_ANALYTICS: `${wsBase}/ide/observability/execution-analytics`
      },
      SEMANTIC: {
        ROOT: `${wsBase}/ide/semantic`
      },
      WORLD_MODEL: {
        ROOT: `${wsBase}/ide/world-model`
      },
      EXPERIMENTS: {
        ROOT: `${wsBase}/ide/experiments`,
        DETAIL: (name: string) => `${wsBase}/ide/experiments/${encodeURIComponent(name)}`
      },
      EDGE: {
        ROOT: `${wsBase}/ide/edge`,
        DASHBOARD: `${wsBase}/ide/edge`,
        PLAYBACK: `${wsBase}/ide/edge/playback`,
        DETECTIONS: `${wsBase}/ide/edge/detections`,
        TOPOLOGY: `${wsBase}/ide/edge/topology`,
        DEVICES: `${wsBase}/ide/edge/devices`,
        BOX: (boxId: string) => `${wsBase}/ide/edge/boxes/${boxId}`,
        ROLLOUTS: `${wsBase}/ide/edge/rollouts`,
        ROLLOUT: (planId: string) => `${wsBase}/ide/edge/rollouts/${planId}`,
        AUDIT: `${wsBase}/ide/edge/audit`,
        PACK: `${wsBase}/ide/edge/pack`
      }
    },

    CONTEXT_GRAPH: `${wsBase}/context-graph`,
    /** The read-only world model a customer opens from the home. The Factory's
     *  own, with its exploration tools, is `IDE.WORLD_MODEL`. */
    WORLD_MODEL: `${wsBase}/world-model`,
    /** The setup wizard. `setup`, not `onboarding`: `/<org>/onboarding` is the
     *  org's own retired wizard, which redirects to the org root. */
    ONBOARDING: `${wsBase}/setup`
  };
}

const ROUTES = {
  ROOT: "/",
  ONBOARDING: "/onboarding",
  /** Managing the store tablet this browser is — or saying it is not one. */
  KIOSK: "/kiosk",
  /** The signed-in person's notifications, across every org: one inbox per person. */
  NOTIFICATIONS: "/notifications",
  /** Partner console — a partner's home. Mirrors the ADMIN surface. */
  PARTNERS: {
    ROOT: "/partners",
    APPS: "/partners/apps",
    TEAM: "/partners/team",
    ACTIVITY: "/partners/activity"
  },

  AUTH: {
    LOGIN: "/login",
    GOOGLE_CALLBACK: "/auth/google/callback",
    OKTA_CALLBACK: "/auth/okta/callback",
    MAGIC_LINK_CALLBACK: "/auth/magic-link/callback",
    GITHUB_AUTH_CALLBACK: "/auth/github/callback",
    /** Dev-only bypass; mounted only when `authConfig.dev_login` is true. */
    DEV_LOGIN: "/dev-login",
    /** API-token sign-in: redeems the one-time ticket in the URL fragment. */
    TOKEN_LOGIN: "/token-login"
  },
  GITHUB: {
    CALLBACK: "/github/callback"
  },

  INVITE: (token: string) => `/invite/${token}`,

  ADMIN: {
    /** The console home — the attention rollup every staff member can open. */
    ROOT: "/admin",
    BILLING_QUEUE: "/admin/billing/queue",
    FEATURE_FLAGS: "/admin/feature-flags",
    INTERNAL_JOBS: "/admin/internal-jobs",
    COMPILES: "/admin/compiles",
    EXPLORER: "/admin/explorer",
    AUDIT: "/admin/audit",
    CUSTOMER_APPS: "/admin/apps",
    AIRHOUSE: "/admin/airhouse",
    APP_ADMINS: "/admin/app-admins",
    /**
     * Every long-lived credential staff can mint or revoke — app publish tokens,
     * sandbox agent tokens, and staff & partner tokens — on one list, filtered
     * by `?kind=`. It replaced three pages that answered one question.
     */
    TOKENS: "/admin/tokens",
    TENANTS: "/admin/tenants",
    /** The tenants directory focused on one entity type. */
    TENANTS_OF: (type: "orgs" | "users" | "partners") => `/admin/tenants?type=${type}`,
    OLTP: "/admin/oltp",
    ORG_DETAIL: (orgId: string) => `/admin/orgs/${orgId}`,
    USER_DETAIL: (userId: string) => `/admin/users/${userId}`,
    WORKSPACES: "/admin/workspaces",
    WORKSPACE_DETAIL: (workspaceId: string) => `/admin/workspaces/${workspaceId}`,
    WORKSPACE_HEALTH: "/admin/workspace-health",
    AIRWAY: "/admin/airway",
    /**
     * What an org costs us and how its apps perform. A fleet list and a
     * per-org console on one route, the shape `/admin/apps` settled on: the
     * list answers "what is out there?" before the console answers "which one
     * thing?".
     */
    OBSERVABILITY: "/admin/observability",
    OBSERVABILITY_ORG: (orgId: string) => `/admin/observability/orgs/${orgId}`,
    /**
     * Last week's custom-app usage — the report the server emails every Monday,
     * shown as a view of Observability rather than a page of its own.
     */
    OBSERVABILITY_WEEKLY: "/admin/observability/weekly",
    /** The signed-in staff member's own preferences. Not a rail entry. */
    SETTINGS: "/admin/settings"
  },

  // Org-scoped routes. Passing an empty `orgSlug` degrades to flat local-mode
  // paths (e.g. `/home` instead of `/acme/home`) — in local
  // mode there's no org and a single implicit workspace, and every call site
  // already defaults to `useCurrentOrg(...).slug ?? ""`, so this makes URL
  // building mode-agnostic without touching the ~100 consumers.
  ORG: (orgSlug: string) => {
    const isLocal = orgSlug === "";
    const base = isLocal ? "" : `/${orgSlug}`;
    return {
      ROOT: isLocal ? "/" : base,
      /** The org's home: every app the person can open across its workspaces. In
       *  local mode there is one workspace, so its own home is the org's. */
      HOME: isLocal ? "/home" : `${base}/home`,
      WORKSPACES: `${base}/workspaces`,
      MEMBERS: `${base}/members`,
      SETTINGS: `${base}/settings`,
      BILLING: {
        CHECKOUT_SUCCESS: `${base}/billing/checkout-success`,
        CHECKOUT_CANCELLED: `${base}/billing/checkout-cancelled`
      },

      /**
       * The org's pages, at `/<org>/<page>`. An org has one workspace, so the
       * address names only the org and the shell loads its workspace
       * (`OrgWorkspace`).
       */
      WORKSPACE: () => (isLocal ? workspaceRoutes("", "") : workspaceRoutes(base, `${base}/home`)),

      /**
       * A link that opens one particular workspace: `/<org>/workspaces/<id>/<page>`
       * remembers it as this org's, then redirects to `/<org>/<page>`
       * (`WorkspaceEntry`). For links made outside that workspace — the admin
       * console, the Factory switcher, Open Factory — while some orgs still have
       * more than one. Inside the shell, use `WORKSPACE`.
       */
      ENTER_WORKSPACE: (wsId: string) => {
        if (isLocal) return workspaceRoutes("", "");
        const entry = `${base}/workspaces/${wsId}`;
        return workspaceRoutes(entry, entry);
      }
    };
  }
} as const;

export default ROUTES;
