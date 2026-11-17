import { isAxiosError } from "axios";
import type { AppDataPage, AppDataRows, AppDataTables } from "@/types/appData";
import { apiClient } from "./axios";
import { environmentParams } from "./customApps";

const dataPath = (id: string) => `/customer-apps/${encodeURIComponent(id)}/data`;

/**
 * A refusal or failure of the data browser, in the server's own words.
 *
 * The routes answer `{ error: <code>, message: <sentence> }`; Axios's own message is
 * "Request failed with status code 403", which `AdminAsync` rightly hides — so the
 * panel would say only "couldn't load", on exactly the refusals an operator most needs
 * explained (a lockdown, a missing role, a read that timed out).
 */
export class AppDataError extends Error {
  readonly code: string | undefined;
  readonly status: number | undefined;

  constructor(message: string, code: string | undefined, status: number | undefined) {
    super(message);
    this.name = "AppDataError";
    this.code = code;
    this.status = status;
  }
}

/** The server's sentence and code, when the body carries them. */
export function toAppDataError(err: unknown): unknown {
  if (!isAxiosError(err)) return err;
  const body: unknown = err.response?.data;
  const fields =
    body && typeof body === "object" ? (body as { error?: unknown; message?: unknown }) : {};
  const message = typeof fields.message === "string" && fields.message ? fields.message : null;
  const code = typeof fields.error === "string" ? fields.error : undefined;
  return message ? new AppDataError(message, code, err.response?.status) : err;
}

async function read<T>(call: () => Promise<{ data: T }>): Promise<T> {
  try {
    return (await call()).data;
  } catch (err) {
    throw toAppDataError(err);
  }
}

/**
 * An app's own OLTP data, read-only, in one environment (production when none is
 * named). Every call that reaches the app's store is written to the audit log.
 */
export const AppDataService = {
  /** The app's tables in `environment`, or why that environment has none. */
  listTables(id: string, environment: string): Promise<AppDataTables> {
    return read(() =>
      apiClient.get(`${dataPath(id)}/tables`, { params: environmentParams(environment) })
    );
  },

  /** One page of one table. */
  listRows(id: string, environment: string, page: AppDataPage): Promise<AppDataRows> {
    return read(() =>
      apiClient.get(`${dataPath(id)}/rows`, {
        params: {
          ...environmentParams(environment),
          table: page.table,
          limit: page.limit,
          offset: page.offset,
          ...(page.order ? { order: page.order.column, dir: page.order.dir } : {})
        }
      })
    );
  }
};
