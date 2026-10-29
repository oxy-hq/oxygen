import type { HomeWidgetsResponse, WidgetPress, WidgetPressResponse } from "@/types/homeWidgets";
import { apiClient } from "./axios";

/** Each app's live cards on the home, and their buttons. Flat routes: the server
 *  decides every app by the caller's access, and runs every function as them. */
export class HomeWidgetsService {
  static async list(orgSlug: string): Promise<HomeWidgetsResponse> {
    const response = await apiClient.get<HomeWidgetsResponse>("/home/widgets", {
      params: { org: orgSlug }
    });
    return response.data;
  }

  static async press({ appId, widget, button }: WidgetPress): Promise<WidgetPressResponse> {
    const path = [appId, widget, "actions", button.action].map(encodeURIComponent).join("/");
    const response = await apiClient.post<WidgetPressResponse>(`/home/widgets/${path}`, {
      input: button.input,
      key: button.key
    });
    return response.data;
  }
}
