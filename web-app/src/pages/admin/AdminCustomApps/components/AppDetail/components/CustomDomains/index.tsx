import { Plus } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import {
  useAddAppDomain,
  useAppDomains,
  useRemoveAppDomain,
  useVerifyAppDomain
} from "@/hooks/api/customApps/useAppDomains";
import type { CustomDomain } from "@/types/apps";
import { LifecycleConfirm } from "../../../AppLifecycle";
import { DomainRow } from "./DomainRow";
import { normalizeDomainInput } from "./dnsRecords";

/**
 * Customer-owned domains for this app — `GET|POST /customer-apps/{id}/domains`,
 * `POST …/{domain_id}/verify`, `DELETE …/{domain_id}`.
 *
 * Add a domain, create the CNAME and TXT records it shows, press Verify. Server
 * errors (a malformed name, a domain another app already verified) arrive as toasts
 * carrying the server's own sentence.
 */
export const CustomDomains = ({ appId }: { appId: string }) => {
  const { data, isLoading, error } = useAppDomains(appId);
  const add = useAddAppDomain(appId);
  const verify = useVerifyAppDomain(appId);
  const remove = useRemoveAppDomain(appId);
  const [input, setInput] = useState("");
  const [removing, setRemoving] = useState<CustomDomain | null>(null);

  if (isLoading) return <Skeleton className='h-20 w-full' />;
  if (error) {
    return (
      <p className='text-destructive text-xs' data-testid='admin-app-domains-error'>
        Couldn't load custom domains.
      </p>
    );
  }

  const domains = data?.domains ?? [];
  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const host = normalizeDomainInput(input);
    if (host) add.mutate(host, { onSuccess: () => setInput("") });
  };

  return (
    <div className='flex flex-col gap-3' data-testid='admin-app-domains'>
      {domains.length > 0 ? (
        <ul className='flex flex-col divide-y divide-border/50 rounded-md border'>
          {domains.map((d) => (
            <DomainRow
              key={d.id}
              domain={d}
              cnameTarget={data?.cname_target ?? null}
              verifying={verify.isPending && verify.variables === d.id}
              onVerify={() => verify.mutate(d.id)}
              onRemove={() => setRemoving(d)}
            />
          ))}
        </ul>
      ) : (
        <p className='text-muted-foreground text-xs' data-testid='admin-app-domains-empty'>
          No custom domain yet.
        </p>
      )}

      <form onSubmit={submit} className='flex items-center gap-2'>
        <Input
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder='app.acme.com'
          className='h-7 max-w-xs font-mono text-xs'
          aria-label='Domain'
          data-testid='admin-app-domain-input'
        />
        <Button
          type='submit'
          size='sm'
          variant='outline'
          className='h-7 gap-1.5 text-xs'
          disabled={!normalizeDomainInput(input) || add.isPending}
          data-testid='admin-app-domain-add'
        >
          <Plus className='size-3.5' />
          {add.isPending ? "Adding…" : "Add domain"}
        </Button>
      </form>
      {/* What adding one does, said before it is done (`custom_domain_host.rs`:
          a custom domain is read as production). The form gave no hint, so "will
          staging move to it?" was answered only by trying. */}
      <p className='text-muted-foreground text-xs' data-testid='admin-app-domain-help'>
        A custom domain serves production only. Staging and sandboxes stay on their platform hosts,
        and the app&rsquo;s existing URLs keep working.
      </p>

      <LifecycleConfirm
        testId='admin-app-domain-confirm'
        copy={
          removing
            ? {
                title: `Remove ${removing.domain}?`,
                description:
                  removing.status === "verified"
                    ? "The app stops loading at this address. Its other addresses keep working."
                    : "The DNS records you created can then be deleted.",
                confirm: "Remove"
              }
            : null
        }
        pending={remove.isPending}
        onConfirm={() =>
          removing && remove.mutate(removing.id, { onSuccess: () => setRemoving(null) })
        }
        onCancel={() => setRemoving(null)}
      />
    </div>
  );
};
