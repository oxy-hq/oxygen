import { isThemeMode, type ThemeMode } from "@/stores/useTheme";
import { apiClient } from "./axios";

/** The person's light/dark mode as the server keeps it: `null` until they choose. */
export interface AppearanceAnswer {
  mode: ThemeMode | null;
}

const read = (data: unknown): AppearanceAnswer => {
  const mode = (data as { mode?: unknown } | null)?.mode;
  return { mode: isThemeMode(mode) ? mode : null };
};

/**
 * The person's light/dark mode, which Oxygen and every custom app follow
 * (`GET`/`PUT /api/me/appearance`). Per person, across orgs. A crew sign-in
 * and a server older than the route both answer 404, which the theme sync
 * reads as "keep this browser's own copy".
 */
export class AppearanceService {
  static async get(): Promise<AppearanceAnswer> {
    const response = await apiClient.get("/me/appearance");
    return read(response.data);
  }

  static async save(mode: ThemeMode): Promise<AppearanceAnswer> {
    const response = await apiClient.put("/me/appearance", { mode });
    return read(response.data);
  }
}
