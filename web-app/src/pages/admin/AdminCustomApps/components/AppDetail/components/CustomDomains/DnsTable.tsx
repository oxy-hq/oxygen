import { CopyButton } from "../../../UrlActions";
import type { DnsRecord } from "./dnsRecords";

/**
 * The records a customer creates at their DNS host, one row each, every cell
 * copyable. A record with no value (no routing target configured yet) says who to
 * ask instead of showing an empty cell.
 */
export const DnsTable = ({ domainId, records }: { domainId: string; records: DnsRecord[] }) => (
  <div className='overflow-x-auto border' data-testid={`admin-app-domain-dns-${domainId}`}>
    <table className='w-full text-left text-xs'>
      <thead className='border-b text-muted-foreground'>
        <tr>
          <th className='px-2 py-1 font-medium'>Type</th>
          <th className='px-2 py-1 font-medium'>Name</th>
          <th className='px-2 py-1 font-medium'>Value</th>
        </tr>
      </thead>
      <tbody className='divide-y divide-border/50'>
        {records.map((r) => (
          <tr key={r.type} data-testid={`admin-app-domain-record-${domainId}-${r.type}`}>
            <td className='px-2 py-1 font-medium font-mono'>{r.type}</td>
            <td className='px-2 py-1'>
              <Copyable
                value={r.name}
                label={`${r.type} name`}
                testId={`${domainId}-${r.type}-name`}
              />
            </td>
            <td className='px-2 py-1'>
              {r.value ? (
                <Copyable
                  value={r.value}
                  label={`${r.type} value`}
                  testId={`${domainId}-${r.type}-value`}
                />
              ) : (
                <span
                  className='text-muted-foreground'
                  data-testid={`admin-app-domain-no-target-${domainId}`}
                >
                  Ask Oxy ops for the routing target
                </span>
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  </div>
);

const Copyable = ({ value, label, testId }: { value: string; label: string; testId: string }) => (
  <span className='flex items-center gap-1'>
    <span className='min-w-0 break-all font-mono'>{value}</span>
    <CopyButton value={value} label={label} testId={`admin-app-domain-copy-${testId}`} />
  </span>
);
