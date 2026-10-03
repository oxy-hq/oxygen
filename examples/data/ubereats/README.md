# UberEats sample report

`2026.08 UberEats Payment Details.csv` — a synthetic payment-details export for
testing the upload endpoint and the pipeline beside it
(`examples/pipelines/ubereats.airway.yml`).

Generated from the source's own `COLUMN_MAP`, so it carries all **49** columns
in report order and cannot drift from what the parser accepts. Verified by
running the shipped `parse_report` over it: 6 rows load, 1 is skipped.

## What each row exercises

| row | why it is there |
| --- | --- |
| 2 ordinary orders | the common case |
| a refund | negative sales, and an **order date in the prior month** — the JE aggregates by the month that PAID, which is why the period is stamped rather than read from Order Date |
| an ad-spend line | **no Order ID at all**, which is why the report has no natural per-row key and `_row_uid` is position-based |
| a second in-scope store | multi-store reports |
| `Someone Else BBQ` | **out of scope** — skipped with a counted warning, not an error. This is what an API section spanning other tenancies looks like |
| a row with a **blank Store ID** | real reports have these, which is why scoping keys on the store NAME and `Store ID` is not a JE-critical column |

Row 0 is Uber's verbose **description** row and row 1 is the short column
names. That is deliberate: the source finds the header by looking for
`Total payout` rather than assuming an offset, and a sample without the
description row would never exercise that path.

Every money cell carries a number rather than a blank. A blank is a documented
*placeholder* meaning "no value", which is a different thing from zero — and
the parser treats them differently.

## Verifying it locally

Self-contained: no file outside the repo, and every value shown.

**1. A bucket.** MinIO stands in for S3.

```bash
docker run -d --name oxy-source-uploads-minio -p 9000:9000 -p 9001:9001 \
  -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
  ghcr.io/oxy-hq/minio:RELEASE.2026-09-22T19-25-18Z server /tmp/data --console-address ":9001"

docker exec oxy-source-uploads-minio sh -c \
  'mc alias set local http://127.0.0.1:9000 minioadmin minioadmin && \
   mc mb -p local/oxy-local-source-uploads'
```

`docker-compose.airhouse.yml` already binds `9000`/`9001`, so if that stack is
up, publish this one elsewhere (`-p 9100:9000 -p 9101:9001`) and point
`AWS_ENDPOINT_URL` at the same port. Or just reuse its MinIO — only the bucket
has to exist.

**2. The environment.** Both halves — see `.env.example` for why the two
sides read different variables.

```bash
export OXY_SOURCE_UPLOAD_ZONE=s3://oxy-local-source-uploads
export AWS_ENDPOINT_URL=http://127.0.0.1:9000
export AWS_ACCESS_KEY_ID=minioadmin AWS_SECRET_ACCESS_KEY=minioadmin
export AWS_REGION=us-east-1
export AWS_ALLOW_HTTP=true          # extract only; without it the run fails
```

**3. Point the pipeline at your workspace.** `base_path` must equal
`<zone>/<workspace_id>/<kind>/<pipeline>` exactly — the server refuses an
upload whose pipeline reads anywhere else, and names both values when they
differ, so a wrong guess here tells you the right answer. The `<pipeline>`
segment is this file's relative path with `/` → `__`, hence
`pipelines__ubereats`. Under `oxy serve --local` the workspace id is the
all-zeros UUID, which is what the example ships with.

**4. A destination.** The example ships `database: local-acme`, which is
`airhouse_managed` — the realistic target, and it needs an airhouse tenant,
so a standalone box cannot run it (`not a known config.yml database with an
airway-writable type`). For a self-contained check, point it at the
`postgres` entry already in `examples/config.yml` and run one to match:

```bash
docker run -d --name oxy-source-uploads-dest -p 5432:5432 \
  -e POSTGRES_USER=admin -e POSTGRES_PASSWORD=admin -e POSTGRES_DB=default postgres:18-alpine
export POSTGRES_PASSWORD=admin        # config.yml reads it via password_var

# in examples/pipelines/ubereats.airway.yml, for local verification only:
#   database: local-acme   ->   database: postgres
```

**5. Run it.**

```bash
cargo run -p oxy-server -- serve --local        # then use the UI
# …or drive the pipeline directly, once a report is in the zone:
cargo run -p oxy-server -- airway run pipelines/ubereats.airway.yml
```

Drop `2026.08 UberEats Payment Details.csv` on the pipeline page's
**Reports** tab. The name carries its own period, so leave the period field
blank.

### What correct looks like

| step | expected |
| --- | --- |
| upload response | `rows: 6` — **not 7**. 7 means the pipeline's `allowed_stores` was not applied, so the count is promising a row the load will drop |
| object lands at | `<ws>/ubereats/pipelines__ubereats/2026.08/<sha>.csv` |
| run logs | `skipped rows for out-of-scope store(s) rows=1 stores=["Someone Else BBQ"]` |
| run result | `table_loaded rows=6` |
| destination | 6 rows, 2 stores — `Poke House SF`, `Poke House Oakland` |

Re-dropping the same file is a no-op by design: the object name is a hash of
the bytes, so it lands on the same key and merges.

### If it fails

| symptom | cause |
| --- | --- |
| `list … failed: request failed` | `AWS_ALLOW_HTTP` unset (fails instantly), or an airway older than 0.1.31 (fails after ~5s — it ignored every `AWS_*` variable and went to EC2 instance metadata) |
| upload says `rows: 7` | scoping not applied — the pipeline read is not reaching `allowed_stores` |
| `base_path` mismatch 400 | the error names both values; copy the expected one |
| upload 503s | `OXY_SOURCE_UPLOAD_ZONE` unset |

**Requires airway ≥ 0.1.31.** Before it the extract could not work in any
environment — `object_store::parse_url` reads no configuration at all, so
the endpoint, credentials and region were dropped and the region defaulted
to `us-east-1`, which also fails a real bucket in any other region.

