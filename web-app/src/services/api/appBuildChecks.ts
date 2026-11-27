import type { BuildChecks, BuildChecksEnvironment, BuildChecksRerun } from "@/types/appBuildChecks";
import { apiClient } from "./axios";

/** `build` is the build's row id or its `build_id`; the server resolves either. */
const checksPath = (appId: string, build: string) =>
  `/customer-apps/${encodeURIComponent(appId)}/builds/${encodeURIComponent(build)}/checks`;

/** Which attempt a file belongs to. */
export interface BuildCheckFileRef {
  attempt: number;
  environment: BuildChecksEnvironment | string;
}

export class AppBuildChecksService {
  /** The latest attempt's checks of one build. A build never checked answers
   *  `state: "none"` with no rows. Staging is asked for as it always was (no
   *  parameter); `production` names the attempt run on the build production serves. */
  static async get(
    appId: string,
    build: string,
    environment?: BuildChecksEnvironment
  ): Promise<BuildChecks> {
    const response =
      environment === "production"
        ? await apiClient.get<BuildChecks>(checksPath(appId, build), { params: { environment } })
        : await apiClient.get<BuildChecks>(checksPath(appId, build));
    return response.data;
  }

  /** Queue a new attempt. Refused with a 409 when staging does not serve the build
   *  (`build_not_on_staging`) or its checks are still running (`checks_running`). */
  static async rerun(appId: string, build: string): Promise<BuildChecksRerun> {
    const response = await apiClient.post<BuildChecksRerun>(
      `${checksPath(appId, build)}/rerun`,
      {}
    );
    return response.data;
  }

  /** One file an attempt's runner uploaded (a screen's picture, a diff). Fetched
   *  through the API client so the session rides along; there is no public URL. */
  static async file(
    appId: string,
    build: string,
    name: string,
    at: BuildCheckFileRef
  ): Promise<Blob> {
    const response = await apiClient.get<Blob>(
      `${checksPath(appId, build)}/files/${encodeURIComponent(name)}`,
      { params: { attempt: at.attempt, environment: at.environment }, responseType: "blob" }
    );
    return response.data;
  }

  /** A person marks the changed screens of one attempt seen. 409 `nothing_to_see`
   *  when no screen changed. The answer is not read: the checks are re-read. */
  static async markChangesSeen(appId: string, build: string, attempt: number): Promise<void> {
    await apiClient.post(`${checksPath(appId, build)}/changes/seen`, { attempt });
  }
}
