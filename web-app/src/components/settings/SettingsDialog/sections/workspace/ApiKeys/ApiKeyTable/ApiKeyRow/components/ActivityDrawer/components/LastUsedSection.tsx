import type React from "react";
import type { ApiKeyLastUsed } from "@/types/apiKey";
import { RelativeTime } from "./RelativeTime";

const Field: React.FC<{ label: string; value?: string | null; mono?: boolean }> = ({
  label,
  value,
  mono
}) => (
  <>
    <dt className='text-muted-foreground'>{label}</dt>
    <dd className={mono ? "break-all font-mono" : "break-words"}>{value || "Not recorded"}</dd>
  </>
);

/** When the key was last used, and from where. */
const LastUsedSection: React.FC<{ lastUsed: ApiKeyLastUsed | null }> = ({ lastUsed }) => (
  <section className='flex flex-col gap-2' data-testid='api-key-activity-last-used'>
    <h3 className='font-medium text-xs'>Last used</h3>
    {lastUsed ? (
      <div className='flex flex-col gap-2 text-xs'>
        <RelativeTime iso={lastUsed.at} className='font-medium text-sm' />
        <dl className='grid grid-cols-[auto_1fr] gap-x-4 gap-y-1'>
          <Field label='IP address' value={lastUsed.ip} mono />
          <Field label='Client' value={lastUsed.user_agent} />
          <Field label='Route' value={lastUsed.route} mono />
        </dl>
      </div>
    ) : (
      <p className='text-muted-foreground text-xs'>Never used.</p>
    )}
  </section>
);

export default LastUsedSection;
