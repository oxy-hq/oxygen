import { formatDistanceToNowStrict } from "date-fns";
import { GitBranch, GitCommitHorizontal } from "lucide-react";
import useServedRevision, { isNothingCompiled } from "@/hooks/api/revisionFiles/useServedRevision";
import type { ServedRevision } from "@/services/api/revisionFiles";
import { githubCommitUrl } from "../githubRepoUrl";

/**
 * Whether `sha` is a git commit. A workspace compiled from a directory with no
 * git (`oxy seed`, single-process mode) records `local-<uuid>` instead.
 */
export const isCommitSha = (sha: string) => /^[0-9a-f]{7,64}$/i.test(sha);

/** "3 hours ago", or "" for a missing or unreadable time. */
export function compiledAgo(compiledAt: string | null, now = new Date()): string {
  if (!compiledAt) return "";
  const at = new Date(compiledAt);
  if (Number.isNaN(at.getTime())) return "";
  return at > now ? "just now" : formatDistanceToNowStrict(at, { addSuffix: true });
}

/**
 * What the Factory is showing: the branch and commit the served revision was
 * compiled from, and when. It replaces the IDE's branch switcher; changing it
 * is a push to the repository.
 */
export default function RevisionInfo({ remoteUrl }: { remoteUrl?: string | null }) {
  const revision = useServedRevision();
  if (revision.isPending) return null;
  if (revision.isError) {
    // 503 is "nothing compiled yet" (the hook polls for the first compile);
    // anything else is the server failing, which must not read as that.
    return isNothingCompiled(revision.error) ? (
      <span className='text-muted-foreground text-xs' data-testid='ide-revision-none'>
        Nothing compiled yet
      </span>
    ) : (
      <span className='text-muted-foreground text-xs' data-testid='ide-revision-unavailable'>
        Revision unavailable
      </span>
    );
  }
  return <RevisionFacts revision={revision.data} remoteUrl={remoteUrl} />;
}

function RevisionFacts({
  revision,
  remoteUrl
}: {
  revision: ServedRevision;
  remoteUrl?: string | null;
}) {
  const short = revision.git_sha.slice(0, 7);
  const commitUrl = isCommitSha(revision.git_sha)
    ? githubCommitUrl(remoteUrl, revision.git_sha)
    : null;
  const ago = compiledAgo(revision.compiled_at);
  const title = `Serving ${revision.kind} revision ${revision.revision_id}: commit ${revision.git_sha}, ${revision.file_count} files`;
  return (
    <div
      className='flex min-w-0 items-center gap-2 text-muted-foreground text-xs'
      title={title}
      data-testid='ide-revision-info'
    >
      {revision.branch && (
        <span className='flex min-w-0 items-center gap-1'>
          <GitBranch className='size-3 shrink-0' aria-hidden />
          <span className='truncate font-mono text-foreground'>{revision.branch}</span>
        </span>
      )}
      <span className='flex items-center gap-1'>
        <GitCommitHorizontal className='size-3 shrink-0' aria-hidden />
        {!isCommitSha(revision.git_sha) ? (
          <span>local files, no commit</span>
        ) : commitUrl ? (
          <a
            href={commitUrl}
            target='_blank'
            rel='noopener noreferrer'
            className='font-mono text-foreground hover:underline'
          >
            {short}
          </a>
        ) : (
          <span className='font-mono text-foreground'>{short}</span>
        )}
      </span>
      {revision.kind !== "main" && (
        <span className='rounded bg-muted px-1.5 py-0.5 text-foreground'>{revision.kind}</span>
      )}
      {ago && <span className='hidden truncate sm:inline'>compiled {ago}</span>}
    </div>
  );
}
