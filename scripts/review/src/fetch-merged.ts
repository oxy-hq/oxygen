// Merged PRs with their comments, reviews and threads, 20 per GraphQL call. The REST
// equivalent is five calls per PR, which spent the shared 5,000-an-hour quota.

import { run } from "./exec.ts";
import type { GhUser, IssueComment, PrMeta, PrReview, ReviewComment } from "./types.ts";

const PR_FIELDS = `
  number title body mergedAt additions deletions headRefOid baseRefName
  baseRepository { defaultBranchRef { name } }
  mergeCommit { oid }
  author { login __typename }
  labels(first: 20) { nodes { name } }
  commits(last: 1) { nodes { commit { committedDate } } }
  comments(first: 100) { nodes { body createdAt author { login __typename } } }
  reviews(first: 50) { nodes { state author { login __typename } } }
  reviewThreads(first: 50) { nodes { comments(first: 20) { nodes {
    databaseId body author { login __typename } replyTo { databaseId } } } } }`;

interface Author {
  login: string;
  __typename: string;
}

/** One pull request as the GraphQL query above returns it. */
export interface PrNode {
  number: number;
  title: string;
  body: string | null;
  mergedAt: string | null;
  additions: number;
  deletions: number;
  headRefOid: string;
  baseRefName: string;
  baseRepository: { defaultBranchRef: { name: string } | null } | null;
  mergeCommit: { oid: string } | null;
  author: Author | null;
  labels: { nodes: { name: string }[] };
  commits: { nodes: { commit: { committedDate: string } }[] };
  comments: { nodes: { body: string; createdAt: string; author: Author | null }[] };
  reviews: { nodes: { state: string; author: Author | null }[] };
  reviewThreads: {
    nodes: {
      comments: {
        nodes: {
          databaseId: number;
          body: string;
          author: Author | null;
          replyTo: { databaseId: number } | null;
        }[];
      };
    }[];
  };
}

export interface Fetched {
  pr: PrMeta;
  comments: IssueComment[];
  reviews: PrReview[];
  reviewComments: ReviewComment[];
  lastCommitAt: string | null;
}

async function graphql<T>(query: string, variables: Record<string, unknown>): Promise<T> {
  const { stdout } = await run("gh", ["api", "graphql", "--input", "-"], {
    input: JSON.stringify({ query, variables })
  });
  const data = JSON.parse(stdout) as { data: T; errors?: { message: string }[] };
  if (data.errors?.length)
    throw new Error(`GraphQL: ${data.errors.map((e) => e.message).join("; ")}`);
  return data.data;
}

const user = (a: Author | null): GhUser | null =>
  a ? { login: a.login, type: a.__typename === "Bot" ? "Bot" : "User" } : null;

/** One GraphQL PR node in the REST shapes the metrics code reads. */
export function fromGraphql(node: PrNode): Fetched {
  const reviewComments = node.reviewThreads.nodes.flatMap((t) =>
    t.comments.nodes.map(
      (c): ReviewComment => ({
        id: c.databaseId,
        in_reply_to_id: c.replyTo?.databaseId ?? null,
        body: c.body,
        user: user(c.author)
      })
    )
  );
  return {
    pr: {
      number: node.number,
      title: node.title,
      body: node.body ?? null,
      draft: false,
      merged_at: node.mergedAt,
      additions: node.additions,
      deletions: node.deletions,
      user: user(node.author) ?? { login: "ghost", type: "User" },
      labels: node.labels.nodes,
      head: { sha: node.headRefOid },
      base: {
        ref: node.baseRefName,
        repo: { default_branch: node.baseRepository?.defaultBranchRef?.name ?? "main" }
      },
      merge_commit_sha: node.mergeCommit?.oid ?? null
    },
    comments: node.comments.nodes.map((c) => ({
      body: c.body,
      created_at: c.createdAt,
      user: user(c.author)
    })),
    reviews: node.reviews.nodes.map((r) => ({ state: r.state, user: user(r.author) })),
    reviewComments,
    lastCommitAt: node.commits.nodes[0]?.commit.committedDate ?? null
  };
}

interface SearchPage {
  search: {
    pageInfo: { hasNextPage: boolean; endCursor: string | null };
    nodes: (PrNode | Record<string, never>)[];
  };
}

const isPr = (n: PrNode | Record<string, never>): n is PrNode => "number" in n;

/** Every PR in `repo` merged on or after `sinceIso`. */
export async function fetchMerged(repo: string, sinceIso: string): Promise<Fetched[]> {
  const q = `repo:${repo} is:pr is:merged merged:>=${sinceIso.slice(0, 10)}`;
  const out: Fetched[] = [];
  let after: string | null = null;
  for (;;) {
    const data: SearchPage = await graphql<SearchPage>(
      `query($q:String!,$after:String){search(query:$q,type:ISSUE,first:20,after:$after){
        pageInfo{hasNextPage endCursor} nodes{... on PullRequest{${PR_FIELDS}}}}}`,
      { q, after }
    );
    out.push(...data.search.nodes.filter(isPr).map(fromGraphql));
    if (!data.search.pageInfo.hasNextPage) return out;
    after = data.search.pageInfo.endCursor;
  }
}

/** Specific PRs by number, in batches of 20 aliased lookups. */
export async function fetchPrs(repo: string, numbers: readonly number[]): Promise<Fetched[]> {
  const [owner = "", name = ""] = repo.split("/");
  const out: Fetched[] = [];
  for (let i = 0; i < numbers.length; i += 20) {
    const batch = numbers.slice(i, i + 20);
    const fields = batch.map((n) => `p${n}: pullRequest(number: ${n}) { ${PR_FIELDS} }`).join("\n");
    const data = await graphql<{ repository: Record<string, PrNode | null> }>(
      `query($owner:String!,$name:String!){repository(owner:$owner,name:$name){${fields}}}`,
      { owner, name }
    );
    for (const node of Object.values(data.repository)) if (node) out.push(fromGraphql(node));
  }
  return out;
}
