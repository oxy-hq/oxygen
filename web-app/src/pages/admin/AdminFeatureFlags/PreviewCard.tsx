import { useState } from "react";
import { Card } from "@/components/ui/shadcn/card";
import { Input } from "@/components/ui/shadcn/input";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useAdminOrgOptions, useFeatureFlagEvaluation } from "@/hooks/api/featureFlags";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import { ADMIN_TONE } from "../components/adminTone";
import { isUuid, type OrgMiss, orgMiss, reasonLabel, resolveOrgId } from "./rules";

function Hint({ children, tone }: { children: React.ReactNode; tone?: "danger" }) {
  return (
    <p
      className={cn(
        "px-4 py-6 text-center text-xs",
        tone ? ADMIN_TONE[tone].text : "text-muted-foreground"
      )}
      data-testid='feature-flag-preview-hint'
    >
      {children}
    </p>
  );
}

/** A typed org name that did not resolve, said without calling a real org absent. */
function OrgMissHint({
  name,
  miss,
  onRetry
}: {
  name: string;
  miss: OrgMiss;
  onRetry: () => void;
}) {
  switch (miss) {
    case "ambiguous":
      return <Hint>More than one org is named “{name}”. Paste its uuid.</Hint>;
    case "loading":
      return <Hint>Loading orgs…</Hint>;
    case "unavailable":
      return (
        <Hint tone='danger'>
          Couldn't load every org, so “{name}” may still exist. Paste its uuid, or{" "}
          <button
            type='button'
            className='rounded-sm underline decoration-current/50 underline-offset-2 outline-none hover:decoration-current focus-visible:ring-2 focus-visible:ring-ring'
            onClick={onRetry}
            data-testid='feature-flag-preview-orgs-retry'
          >
            try again
          </button>
          .
        </Hint>
      );
    case "absent":
      return <Hint>No org named “{name}”.</Hint>;
  }
}

/** What each flag serves a given org (and person). Reads only; records no exposure. */
export function PreviewCard() {
  const orgList = useAdminOrgOptions();
  const { orgs } = orgList;
  const [orgText, setOrgText] = useState("");
  const [userText, setUserText] = useState("");
  const orgId = resolveOrgId(orgText, orgs) ?? undefined;
  const userId = isUuid(userText) ? userText.trim() : undefined;
  const result = useFeatureFlagEvaluation({ orgId, userId });
  const rows = result.data ?? [];
  const asked = !!orgId || !!userId;

  let body: React.ReactNode;
  if (orgText.trim() && !orgId)
    body = (
      <OrgMissHint
        name={orgText.trim()}
        miss={orgMiss(orgText, orgs, orgList)}
        onRetry={orgList.refetch}
      />
    );
  else if (!asked) body = <Hint>Pick an org, or paste a user uuid, to see what it gets.</Hint>;
  else if (result.isError) body = <Hint tone='danger'>Couldn't preview. Try again.</Hint>;
  else if (result.isPending) body = <Hint>Checking…</Hint>;
  else {
    body = (
      <Table>
        <TableHeader>
          <TableRow className={ADMIN_HEADER_ROW_CLASS}>
            <AdminTh className='pl-4'>Flag</AdminTh>
            <AdminTh>Serves</AdminTh>
            <AdminTh>Because</AdminTh>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((r) => (
            <TableRow
              key={r.key}
              className='border-border/60'
              data-testid={`feature-flag-preview-row-${r.key}`}
            >
              <TableCell className='pl-4 font-mono text-xs'>{r.key}</TableCell>
              <TableCell className='font-medium font-mono text-xs'>{r.variant}</TableCell>
              <TableCell
                className={cn(
                  "text-xs",
                  r.reason === "ERROR" ? ADMIN_TONE.danger.text : "text-muted-foreground"
                )}
                title={r.reason}
              >
                {reasonLabel(r.reason, r.bucket)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    );
  }

  return (
    <Card className='overflow-hidden' data-testid='feature-flag-preview'>
      <div className='flex flex-wrap items-end justify-between gap-3 border-border/60 border-b px-4 py-3'>
        <div>
          <h3 className='font-semibold text-sm'>Preview</h3>
          <p className='text-muted-foreground text-xs'>What an org, or one person, gets now.</p>
        </div>
        <div className='flex flex-wrap gap-2'>
          <Input
            value={orgText}
            onChange={(e) => setOrgText(e.target.value)}
            list='feature-flag-preview-orgs'
            placeholder='Org name or uuid'
            aria-label='Org'
            className='h-8 w-56 text-xs'
            data-testid='feature-flag-preview-org'
          />
          <datalist id='feature-flag-preview-orgs'>
            {orgs.map((o) => (
              <option key={o.id} value={o.name} label={o.id} />
            ))}
          </datalist>
          <Input
            value={userText}
            onChange={(e) => setUserText(e.target.value)}
            placeholder='User uuid (optional)'
            aria-label='User uuid'
            className='h-8 w-64 font-mono text-xs'
            data-testid='feature-flag-preview-user'
          />
        </div>
      </div>
      {body}
    </Card>
  );
}
