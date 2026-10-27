import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import HowCalculated from "../../ResultPane/components/HowCalculated";
import Section from "../../Section";
import { groupRows, membershipTotals } from "../operations";

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/**
 * The groups a dimension-assigned design randomised, with the members pinned at
 * the first check. Names are shown as sent; a 300-member group scrolls in its own box.
 */
export default function GroupMembership({
  assignBy,
  membership
}: {
  assignBy: string;
  membership: Record<string, string[]> | null;
}) {
  const rows = groupRows(membership);
  const totals = membershipTotals(rows);
  return (
    <Section title={`Groups, by ${assignBy}`} testId='experiment-groups'>
      {membership === null ? (
        <div className='flex flex-wrap items-center gap-2' data-testid='experiment-groups-unpinned'>
          <ToneBadge tone='neutral'>Not pinned yet</ToneBadge>
          <span className='text-muted-foreground text-xs'>
            Membership is pinned by the first check, which has not run yet.
          </span>
        </div>
      ) : (
        <div className='flex flex-col gap-3'>
          <div className='flex flex-wrap items-center gap-2' data-testid='experiment-groups-total'>
            <ToneBadge tone='info'>{count(totals.groups, "group", "groups")}</ToneBadge>
            <ToneBadge tone='neutral'>{count(totals.members, "member", "members")}</ToneBadge>
          </div>
          <HowCalculated title='How this works'>
            <p>
              Pinned at the first check. A unit that appears later is in neither arm; one whose{" "}
              {assignBy} changes stays in the group it was randomised with.
            </p>
          </HowCalculated>
          <HowCalculated title='Show groups and members'>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Group</TableHead>
                  <TableHead>Members</TableHead>
                  <TableHead>Names</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((r) => (
                  <TableRow key={r.group} data-testid={`experiment-group-${r.group}`}>
                    <TableCell>{r.group}</TableCell>
                    <TableCell>{r.members.length}</TableCell>
                    <TableCell>
                      <p className='max-h-40 overflow-auto whitespace-pre-wrap break-words text-xs'>
                        {r.members.join(", ")}
                      </p>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </HowCalculated>
        </div>
      )}
    </Section>
  );
}
