import { apiClient } from "./axios";

export type AppIntegrationKind = "toast" | "openweathermap" | "besttime" | "unifi";

export type AppIntegration =
  | {
      kind: "toast";
      name: string;
      webhook_secret_var: string;
      restaurant_guids: string[];
    }
  | {
      kind: "openweathermap";
      name: string;
      api_key_var: string;
    }
  | {
      kind: "besttime";
      name: string;
      api_key_var: string;
    }
  | {
      kind: "unifi";
      name: string;
      api_key_var: string;
    };

export type UpsertAppIntegrationBody =
  | {
      kind: "toast";
      name: string;
      webhook_secret_var: string;
      restaurant_guids: string[];
    }
  | {
      kind: "openweathermap";
      name: string;
      api_key_var: string;
    }
  | {
      kind: "besttime";
      name: string;
      api_key_var: string;
    }
  | {
      kind: "unifi";
      name: string;
      api_key_var: string;
    };

export const AppIntegrationsService = {
  async list(projectId: string): Promise<AppIntegration[]> {
    const response = await apiClient.get<AppIntegration[]>(`/${projectId}/app-integrations`);
    return response.data;
  }
};
