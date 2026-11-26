import { AlertTriangle, GitBranch } from "lucide-react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";

/** Normalize a git remote URL (`git@host:org/repo.git`, `https://…`, `ssh://…`)
 *  to an https browse URL, or null when unrecognized. */
function repoBrowseUrl(remote: string): string | null {
  const s = remote.trim();
  const ssh = s.match(/^git@([^:]+):(.+?)(?:\.git)?$/);
  if (ssh) return `https://${ssh[1]}/${ssh[2]}`;
  try {
    const u = new URL(s.replace(/^ssh:\/\//, "https://"));
    return `https://${u.host}${u.pathname.replace(/\.git$/, "")}`;
  } catch {
    return null;
  }
}

/** A GitHub-style source link: branch + short commit, linking to the commit
 *  (Vercel-style). Silent when the repo URL can't be normalized. */
export const SourceLink = ({
  repo,
  sha,
  branch
}: {
  repo: string;
  sha: string;
  branch?: string | null;
}) => {
  const base = repoBrowseUrl(repo);
  const label = `${branch ? `${branch} · ` : ""}${sha.slice(0, 7)}`;
  const content = (
    <>
      <GitBranch className='size-3 shrink-0' />
      <span className='truncate'>{label}</span>
    </>
  );
  if (!base) {
    return (
      <span className='inline-flex items-center gap-1 text-muted-foreground/70 text-xs'>
        {content}
      </span>
    );
  }
  return (
    <a
      href={`${base}/commit/${sha}`}
      target='_blank'
      rel='noopener noreferrer'
      title={`${branch ? `${branch} @ ` : ""}${sha}`}
      onClick={(e) => e.stopPropagation()}
      className='inline-flex items-center gap-1 text-muted-foreground/80 text-xs hover:text-foreground hover:underline'
    >
      {content}
    </a>
  );
};

/**
 * Shown in `SourceLink`'s place when a build's repo + commit don't both
 * resolve. Reaching the code needs both: a repo without a commit points at a
 * branch, which moves; a commit without a repo has nowhere to resolve.
 *
 * The silence this replaces was the whole problem: a build with no provenance
 * rendered identically to one whose link simply sat further down the row, so
 * an app nobody could trace looked exactly like an app nobody had scrolled
 * to. `oxyc publish` warns about this at publish time; this is where you find
 * the ones that already shipped.
 *
 * Names which half is missing — this row has both fields, so telling an
 * operator "no source recorded" when the repo is right there would send them
 * looking for the wrong thing.
 */
export const IncompleteSource = ({ repo, sha }: { repo?: string | null; sha?: string | null }) => {
  const hasRepo = !!repo?.trim();
  const hasSha = !!sha?.trim();
  const label = hasRepo ? "No commit recorded" : hasSha ? "No repo recorded" : "No source recorded";
  const detail = hasRepo
    ? "This build records a git repo but no commit, so the link points at a branch — which moves, and won't identify the code that is running."
    : hasSha
      ? "This build records a commit but no git repo, so there's nowhere to resolve that sha."
      : "This build records no git repo and no commit, so there's no way to get from the running app back to its code.";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          className={cn("inline-flex w-fit items-center gap-1 text-xs", ADMIN_TONE.warn.text)}
          data-testid='admin-app-build-no-source'
        >
          <AlertTriangle className='size-3 shrink-0' />
          <span>{label}</span>
        </span>
      </TooltipTrigger>
      <TooltipContent className={`${DESIGN_REFERENCE} max-w-xs`}>
        {detail} Re-publish from the app's git checkout to attach provenance.
      </TooltipContent>
    </Tooltip>
  );
};
