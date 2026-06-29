-- Phase 4 WS-D — telemetry events table for the Postgres-backed TelemetrySink.
-- Applied by gonedark_server::postgres::PgTelemetrySink::migrate (sqlx embedded migrator).
-- Only reached AFTER the consent gate (see crate::telemetry::ingest) — schema holds no PII
-- by construction: `properties` is opaque client JSON and no column identifies a person.

CREATE TABLE IF NOT EXISTS telemetry_events (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- Client-minted idempotency key; UNIQUE so re-delivery (ON CONFLICT DO NOTHING) dedupes.
    event_id    TEXT        NOT NULL UNIQUE,
    -- Typed EventKind serialized via EventKind::as_db_str (snake_case, matches the JSON wire).
    kind        TEXT        NOT NULL,
    -- Opaque client wall-clock (ISO-8601 string); the scaffold does not parse it.
    client_ts   TEXT        NOT NULL,
    -- Kind-specific detail; opaque JSON the client controls. Defaults to {}.
    properties  JSONB       NOT NULL DEFAULT '{}'::jsonb,
    -- Server receipt time, for retention/ordering.
    received_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Common query axis: events of a kind over time.
CREATE INDEX IF NOT EXISTS telemetry_events_kind_received_at_idx
    ON telemetry_events (kind, received_at);
