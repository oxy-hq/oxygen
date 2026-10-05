import type { Artifact } from "@/types/artifact";
import { apiClient } from "./axios";

export class ChartService {
  static async getChart(projectId: string, branchName: string, file_path: string): Promise<string> {
    const response = await apiClient.get(`/${projectId}/charts/${file_path}`, {
      params: {
        branch: branchName
      }
    });
    return response.data;
  }
}

export class ArtifactService {
  // No branch: an artifact is one Postgres row found by its id, the same on
  // every branch. A `?branch=` here would buy nothing and send the read to the
  // one node that owns the workspace files.
  static async getArtifact(projectId: string, id: string): Promise<Artifact> {
    const response = await apiClient.get(`/${projectId}/artifacts/${id}`);
    return response.data;
  }
}

// The server also sends `builder_path`, always null since the path-based builder was
// removed; it stays in the response for older clients, and nothing here reads it.
export interface BuilderAvailability {
  available: boolean;
  /** True when the built-in copilot is configured. */
  builtin?: boolean;
  /** Model name for the built-in copilot. */
  model?: string;
}

export class BuilderService {
  static async checkBuilderAvailability(projectId: string): Promise<BuilderAvailability> {
    const response = await apiClient.get(`/${projectId}/builder-availability`);
    return response.data;
  }
}
