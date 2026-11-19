import type { Artifact } from "@/types/artifact";
import { apiClient } from "./axios";

export class ChartService {
  static async getChart(projectId: string, file_path: string): Promise<string> {
    const response = await apiClient.get(`/${projectId}/charts/${file_path}`);
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
