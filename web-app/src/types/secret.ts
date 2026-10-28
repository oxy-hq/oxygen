export interface Secret {
  id: string;
  name: string;
  description?: string;
  created_at: string;
  updated_at: string;
  created_by: string;
  created_by_email?: string;
  updated_by_email?: string;
  is_active: boolean;
}

export interface CreateSecretRequest {
  name: string;
  value: string;
  description?: string;
}

export interface BulkCreateSecretsRequest {
  secrets: CreateSecretRequest[];
}

export interface BulkCreateSecretsResponse {
  created_secrets: CreateSecretResponse[];
  failed_secrets: {
    secret: CreateSecretRequest;
    error: string;
  }[];
}

export interface CreateSecretResponse {
  id: string;
  name: string;
  description?: string;
  created_at: string;
  updated_at: string;
  created_by: string;
  is_active: boolean;
}

export interface UpdateSecretRequest {
  value?: string;
  description?: string;
}

export interface SecretListResponse {
  secrets: Secret[];
  total: number;
}

export interface SecretFormData {
  name: string;
  value: string;
  description?: string;
}

export interface SecretEditFormData {
  value?: string;
  description?: string;
}
