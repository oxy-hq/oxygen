-- One row per lease, always the lease as it stands now. The app never keeps
-- history itself: `oltpChanges` in oxy-app.json ships every version of every
-- row to the workspace's Airhouse.
create table leases (
  lease_ref    text   primary key,
  unit         text   not null,
  tenant_name  text   not null,
  -- Personal data: excluded from the change stream, so it never reaches
  -- Airhouse and keeps no history there.
  tenant_email text,
  rent_cents   bigint not null check (rent_cents > 0),
  starts_on    date   not null,
  ended_on     date
);
