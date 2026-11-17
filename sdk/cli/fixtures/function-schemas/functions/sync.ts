// A handler exported by name, an arrow-free body cast with `req` renamed.
type SyncBody = { since: string; full?: boolean };

async function sync(request: { body: string }, _ctx: unknown) {
  const body = JSON.parse(request.body) as SyncBody;
  return { synced: body.full ? "all" : body.since };
}

export default sync;
