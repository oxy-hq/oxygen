// Outside the subset: an intersection. Publish warns and ships no inputSchema.
interface Base {
  id: string;
}

export type Input = Base & { extra: string };

export default async function tangled(_req: unknown, _ctx: unknown): Promise<void> {}
