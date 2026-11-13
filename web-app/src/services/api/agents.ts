import { apiClient } from "./axios";

/// Lightweight listing entry returned by `GET /{workspaceId}/agents`.
///
/// Returns agentic (`.agentic.yml`) + analytics-automation (`.aw.yml`)
/// agents — the chat panel's selector is the only consumer.
export type AgentInfo = {
  /// Display name (parsed from the file or `llm.ref` snippet).
  name: string;
  /// Workspace-relative path to the agent file.
  path: string;
  /// Whether the agent is exposed publicly. Always `true` for agentic agents.
  public: boolean;
  /// Model ref this agent resolves through.
  model?: string;
  /// IANA timezone (e.g. `America/Los_Angeles`) the agent resolves relative
  /// dates in. Surfaced so the workspace clock can show local time.
  timezone?: string;
};

export const AgentService = {
  async listAgents(projectId: string): Promise<AgentInfo[]> {
    const res = await apiClient.get<AgentInfo[]>(`/${projectId}/agents`);
    return res.data;
  }
};
