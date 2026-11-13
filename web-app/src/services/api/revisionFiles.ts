import { apiClient } from "./axios";

/** What a compiled file is (`oxy::config::RevisionFileKind`). */
export type RevisionFileKind =
  | "config"
  | "agent"
  | "view"
  | "topic"
  | "automation"
  | "query"
  | "migration"
  | "pipeline"
  | "simulation"
  | "context_document"
  | "monitor"
  | "reconcile"
  | "world_model";

export interface RevisionFileEntry {
  path: string;
  kind: RevisionFileKind;
}

export interface RevisionFile extends RevisionFileEntry {
  language: "yaml" | "sql" | "markdown";
  content: string;
}

/** The revision a request reads and its commit (`oxy::config::ServedRevision`). */
export interface ServedRevision {
  revision_id: string;
  git_sha: string;
  branch: string | null;
  /** `main`, or a preview/staging kind. */
  kind: string;
  /** RFC 3339; absent while compiling. */
  compiled_at: string | null;
  file_count: number;
}

/**
 * The served revision's files, read-only. The IDE's tree read the working copy
 * (`GET /files`), which went with the IDE; these read the compiled rows, and a
 * pinned preview answers from its own revision through the request header.
 */
export class RevisionFilesService {
  /** The revision this workspace's requests read, for the Factory's top bar. */
  static async served(projectId: string): Promise<ServedRevision> {
    const response = await apiClient.get(`/${projectId}/revision`);
    return response.data;
  }

  static async list(projectId: string): Promise<RevisionFileEntry[]> {
    const response = await apiClient.get(`/${projectId}/revision/files`);
    return response.data;
  }

  static async get(projectId: string, path: string): Promise<RevisionFile> {
    const response = await apiClient.get(`/${projectId}/revision/files/content`, {
      params: { path }
    });
    return response.data;
  }
}
