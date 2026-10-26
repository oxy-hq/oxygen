// The GitHub calls the pipeline makes, through `gh api` so auth is GH_TOKEN or the
// local `gh` login. Thin on purpose: logic lives in the pure modules. Response shapes
// are asserted at this boundary, the one place they enter the program.

import { run } from "./exec.ts";
import type {
  CommitStatus,
  IssueComment,
  PrMeta,
  PrReview,
  ReviewComment,
  Thread
} from "./types.ts";

interface ApiOptions {
  method?: "GET" | "POST" | "PATCH";
  body?: unknown;
  paginate?: boolean;
}

async function api<T>(
  path: string,
  { method = "GET", body, paginate = false }: ApiOptions = {}
): Promise<T> {
  const args = ["api", path, "-H", "Accept: application/vnd.github+json"];
  if (method !== "GET") args.push("--method", method);
  if (paginate) args.push("--paginate", "--slurp");
  if (body !== undefined) args.push("--input", "-");
  const { stdout } = await run(
    "gh",
    args,
    body === undefined ? {} : { input: JSON.stringify(body) }
  );
  if (!stdout.trim()) return null as T;
  const data: unknown = JSON.parse(stdout);
  return (paginate && Array.isArray(data) ? data.flat() : data) as T;
}

const list = <T>(path: string): Promise<T[]> => api<T[]>(path, { paginate: true });

interface GraphqlResponse<T> {
  data: T;
  errors?: { message: string }[];
}

async function graphql<T>(query: string, variables: Record<string, unknown>): Promise<T> {
  const { stdout } = await run("gh", ["api", "graphql", "--input", "-"], {
    input: JSON.stringify({ query, variables })
  });
  const data = JSON.parse(stdout) as GraphqlResponse<T>;
  if (data.errors?.length)
    throw new Error(`GraphQL: ${data.errors.map((e) => e.message).join("; ")}`);
  return data.data;
}

const ownerAndName = (repo: string): { owner: string; name: string } => {
  const [owner = "", name = ""] = repo.split("/");
  return { owner, name };
};

interface PrNode {
  number: number;
  title: string;
  body: string | null;
  isDraft: boolean;
  headRefOid: string;
  headRefName: string;
  baseRefOid: string;
  baseRefName: string;
  author: { login: string; __typename: string } | null;
  labels: { nodes: { name: string }[] };
}

/**
 * The PR fields the pipeline reads, in REST's shape, over GraphQL: its budget is
 * separate from the REST quota that every tool on an account shares.
 */
export async function getPr(repo: string, n: number): Promise<PrMeta> {
  const data = await graphql<{ repository: { pullRequest: PrNode } }>(
    `query($owner:String!,$name:String!,$n:Int!){repository(owner:$owner,name:$name){pullRequest(number:$n){
      number title body isDraft headRefOid headRefName baseRefOid baseRefName author{login __typename} labels(first:50){nodes{name}}}}}`,
    { ...ownerAndName(repo), n }
  );
  const p = data.repository.pullRequest;
  return {
    number: p.number,
    title: p.title,
    body: p.body,
    draft: p.isDraft,
    user: {
      login: p.author?.login ?? "ghost",
      type: p.author?.__typename === "Bot" ? "Bot" : "User"
    },
    labels: p.labels.nodes,
    head: { sha: p.headRefOid, ref: p.headRefName },
    base: { sha: p.baseRefOid, ref: p.baseRefName }
  };
}

export const listIssueComments = (repo: string, n: number) =>
  list<IssueComment>(`repos/${repo}/issues/${n}/comments?per_page=100`);
export const listReviewComments = (repo: string, n: number) =>
  list<ReviewComment>(`repos/${repo}/pulls/${n}/comments?per_page=100`);
export const listReviews = (repo: string, n: number) =>
  list<PrReview>(`repos/${repo}/pulls/${n}/reviews?per_page=100`);
export const listPrFiles = (repo: string, n: number) =>
  list<{ filename: string; previous_filename?: string }>(
    `repos/${repo}/pulls/${n}/files?per_page=100`
  );

export const createIssueComment = (repo: string, n: number, body: string) =>
  api<IssueComment>(`repos/${repo}/issues/${n}/comments`, { method: "POST", body: { body } });
export const updateIssueComment = (repo: string, id: number, body: string) =>
  api<IssueComment>(`repos/${repo}/issues/comments/${id}`, { method: "PATCH", body: { body } });

export const replyToReviewComment = (repo: string, n: number, commentId: number, body: string) =>
  api<ReviewComment>(`repos/${repo}/pulls/${n}/comments/${commentId}/replies`, {
    method: "POST",
    body: { body }
  });
export const createReview = (repo: string, n: number, review: Record<string, unknown>) =>
  api<{ id: number }>(`repos/${repo}/pulls/${n}/reviews`, { method: "POST", body: review });

export const setStatus = (
  repo: string,
  sha: string,
  status: CommitStatus & { context: string; target_url?: string | undefined }
) => api<unknown>(`repos/${repo}/statuses/${sha}`, { method: "POST", body: status });

/** The merge base of a PR: what its diff is against, which the base branch tip is not. */
export const mergeBase = (repo: string, base: string, head: string) =>
  api<{ merge_base_commit: { sha: string } }>(
    `repos/${repo}/compare/${encodeURIComponent(base)}...${encodeURIComponent(head)}`
  ).then((r) => r.merge_base_commit.sha);

export const getFile = (repo: string, path: string, ref: string) =>
  api<{ content: string }>(`repos/${repo}/contents/${path}?ref=${encodeURIComponent(ref)}`).then(
    (r) => Buffer.from(r.content, "base64").toString("utf8")
  );

interface ThreadPage {
  repository: {
    pullRequest: {
      reviewThreads: {
        pageInfo: { hasNextPage: boolean; endCursor: string | null };
        nodes: { id: string; isResolved: boolean; comments: { nodes: { databaseId: number }[] } }[];
      };
    };
  };
}

/** Review threads with the database id of their first comment, to resolve them later. */
export async function listReviewThreads(repo: string, n: number): Promise<Thread[]> {
  const threads: Thread[] = [];
  let after: string | null = null;
  for (;;) {
    const data: ThreadPage = await graphql<ThreadPage>(
      `query($owner:String!,$name:String!,$n:Int!,$after:String){repository(owner:$owner,name:$name){pullRequest(number:$n){
        reviewThreads(first:100,after:$after){pageInfo{hasNextPage endCursor}
          nodes{id isResolved comments(first:1){nodes{databaseId}}}}}}}`,
      { ...ownerAndName(repo), n, after }
    );
    const page = data.repository.pullRequest.reviewThreads;
    for (const t of page.nodes)
      threads.push({
        id: t.id,
        resolved: t.isResolved,
        firstComment: t.comments.nodes[0]?.databaseId
      });
    if (!page.pageInfo.hasNextPage) return threads;
    after = page.pageInfo.endCursor;
  }
}

export const resolveThread = (threadId: string) =>
  graphql<unknown>(
    `mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}`,
    { id: threadId }
  );

export const minimizeComment = (nodeId: string) =>
  graphql<unknown>(
    `mutation($id:ID!){minimizeComment(input:{subjectId:$id,classifier:OUTDATED}){minimizedComment{isMinimized}}}`,
    {
      id: nodeId
    }
  );
