// Shared by the usage-report hook tests. Not imported by anything that ships.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type {
  UsageReportEmailPreference,
  UsageReportRecipient,
  UsageReportRecipientsResponse
} from "@/types/usageReport";
import queryKeys from "../queryKey";

export const OWN_KEY = queryKeys.usageReport.emailPreference();
export const LIST_KEY = queryKeys.usageReport.recipients();

export const ownPreference: UsageReportEmailPreference = {
  email: "luong@oxy.tech",
  enabled: true,
  delivery: "email"
};

export const person = (over: Partial<UsageReportRecipient> = {}): UsageReportRecipient => ({
  email: "ada@oxy.tech",
  role: "global_admin",
  scope_all: true,
  org_count: null,
  enabled: true,
  is_self: false,
  updated_by: null,
  updated_at: null,
  ...over
});

/** The caller's own row, then two other people. Everyone is emailed. */
export const everyone = (): UsageReportRecipientsResponse => ({
  recipients: [
    person({ email: "luong@oxy.tech", role: "global_owner", is_self: true }),
    person({ email: "ada@oxy.tech" }),
    person({ email: "mel@oxy.tech" })
  ]
});

/**
 * A query client whose caches already hold what the page would have loaded, a wrapper to
 * mount a hook against it, and readers for the two caches.
 */
export function seededClient(seed: {
  own?: UsageReportEmailPreference;
  list?: UsageReportRecipientsResponse;
}) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } }
  });
  if (seed.own) queryClient.setQueryData(OWN_KEY, seed.own);
  if (seed.list) queryClient.setQueryData(LIST_KEY, seed.list);

  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  const own = () => queryClient.getQueryData<UsageReportEmailPreference>(OWN_KEY);
  const list = () => queryClient.getQueryData<UsageReportRecipientsResponse>(LIST_KEY);
  const row = (email: string) => list()?.recipients.find((r) => r.email === email);

  return { queryClient, wrapper, own, list, row };
}

/** A promise the test settles by hand, so a cache can be read while a save is in flight. */
export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
