-- mandate-ledger PostgreSQL schema. Applied by `PostgresStore::migrate`.
-- Every statement is idempotent so migrate() can run on every boot.

CREATE TABLE IF NOT EXISTS ml_schema (
    version    INTEGER PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Event sequence numbers are drawn before the row is written, because the
-- number is part of the hash preimage. nextval is non-transactional, so a
-- rolled-back append leaves a gap. That is fine: evidence verification
-- requires sequence numbers to be strictly increasing, not contiguous.
CREATE SEQUENCE IF NOT EXISTS ml_event_seq AS BIGINT START 1;

-- The append-only, hash-chained log. `body` is the serialized EventBody;
-- `hash` is SHA-256 over RFC 8785 canonical JSON of (seq, ctx, at, body),
-- chained onto `prev_hash`. Hashes are stored as text ("sha256:<hex>") so
-- the chain can be read and checked from a psql session.
CREATE TABLE IF NOT EXISTS ml_events (
    seq       BIGINT PRIMARY KEY,
    ctx       TEXT   NOT NULL,
    at        BIGINT NOT NULL,
    prev_hash TEXT   NOT NULL,
    hash      TEXT   NOT NULL,
    body      JSONB  NOT NULL
);
CREATE INDEX IF NOT EXISTS ml_events_ctx_seq ON ml_events (ctx, seq);
CREATE UNIQUE INDEX IF NOT EXISTS ml_events_hash ON ml_events (hash);

-- Current position of each payment context. Mirrors ml_core::Record.
CREATE TABLE IF NOT EXISTS ml_contexts (
    ctx                  TEXT PRIMARY KEY,
    state                TEXT    NOT NULL,
    mandate_id           TEXT    NOT NULL,
    cart_hash            TEXT    NOT NULL,
    merchant             TEXT    NOT NULL,
    amount               NUMERIC NOT NULL,
    currency             TEXT    NOT NULL,
    rail                 TEXT,
    payment_reference    TEXT,
    idempotency_key      TEXT,
    settlement_reference TEXT,
    failure_reason       TEXT,
    receipt_reference    TEXT
);
CREATE INDEX IF NOT EXISTS ml_contexts_mandate ON ml_contexts (mandate_id);
CREATE INDEX IF NOT EXISTS ml_contexts_state ON ml_contexts (state);

-- Outstanding reservation per mandate: the running total of live
-- authorizations, checked against the mandate's max_total on every append.
CREATE TABLE IF NOT EXISTS ml_reservations (
    mandate_id TEXT PRIMARY KEY,
    amount     NUMERIC NOT NULL,
    currency   TEXT    NOT NULL
);

-- One row per successful authorization, for velocity windows.
CREATE TABLE IF NOT EXISTS ml_authorizations (
    id         BIGSERIAL PRIMARY KEY,
    mandate_id TEXT   NOT NULL,
    ctx        TEXT   NOT NULL,
    at         BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS ml_authorizations_window
    ON ml_authorizations (mandate_id, at DESC);

-- Single-use payment nonces. The primary key is what makes a replayed
-- payment proof impossible: a second context presenting the same
-- (rail, nonce) collides here and is refused.
CREATE TABLE IF NOT EXISTS ml_nonces (
    rail  TEXT NOT NULL,
    nonce TEXT NOT NULL,
    ctx   TEXT NOT NULL,
    PRIMARY KEY (rail, nonce)
);

-- Revoked mandates. Presence is revocation; the row is never removed.
CREATE TABLE IF NOT EXISTS ml_revocations (
    mandate_id TEXT PRIMARY KEY,
    revoked_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO ml_schema (version) VALUES (1) ON CONFLICT DO NOTHING;
