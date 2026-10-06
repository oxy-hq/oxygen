import { apiClient } from "./axios";

/** The most recent invocation that failed this way. */
export interface AppIssueOccurrence {
  invocation_id: string;
  /** `success` here is a function that answered 5xx or caught a failed
   *  `ctx.*` call — counted as a failure all the same. */
  status: string;
  result_status: number | null;
  /** Absent for a caught host-call failure and for a 5xx the handler returned. */
  error: string | null;
  /** The publish's build id of the build it ran on. */
  build_id: string | null;
  created_at: string;
}

/** One distinct failure of one function — the pager's `(function, fingerprint)`. */
export interface AppIssue {
  function_name: string;
  fingerprint: string;
  occurrences: number;
  /** Inside the window: an older failure began earlier than this says. */
  first_seen: string;
  last_seen: string;
  builds: number;
  /** It has happened on the build production serves now. */
  on_live_build: boolean;
  last: AppIssueOccurrence;
}

export interface AppIssueList {
  window_days: number;
  issues: AppIssue[];
  /** More distinct failures occurred in the window than `issues` holds. */
  truncated: boolean;
}

export class AppIssuesService {
  /**
   * An app's production failures of the last `days`, grouped per
   * `(function, fingerprint)`. Derived from invocation rows on every read —
   * nothing about an issue is stored, so there is nothing to mutate here.
   */
  static async list(appId: string, days: number): Promise<AppIssueList> {
    const response = await apiClient.get<AppIssueList>(`/admin/apps/${appId}/issues?days=${days}`);
    return response.data;
  }
}
