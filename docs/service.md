# The service

`ml-serve` is the engine as an HTTP service: one process, one ledger, one
PostgreSQL database, for a host that would rather not embed the engine.
Everything `ml` can do to a ledger, the service does over HTTP, with the
same codes and the same rules, and the two can share a database.

```bash
cargo build -p ml-serve
ML_DATABASE_URL=postgres://localhost/mandate_ledger \
ML_API_KEYS="$(openssl rand -hex 32)" \
ML_TRUSTED_SIGNERS="user:alice=$(jq -r .public alice.key.pub)" \
target/debug/ml-serve
```

## Conventions

- **Status codes mean something**, and mirror `ml`'s exit codes.

  | Status | Meaning |
  |---|---|
  | `200` | The step was allowed; the body states the token's facts. |
  | `409` | The ledger evaluated the step and **refused** it. A decision, not a failure: `code`, `stage`, `detail`, `ctx` and `recorded`. Never retry it. |
  | `503` | The ledger could not decide: the store or the rail was unavailable, every permit was in use, or the call timed out. `Retry-After: 1`. Read the context back first: a step that timed out may still have been recorded. |
  | `400`, `413`, `415`, `422` | Not a request the ledger can be asked: malformed JSON, an unknown field, an id out of bounds, a body too large, the wrong content type. |
  | `401` | No valid API key. |
  | `404` | A read of something that does not exist. Reads only: a lifecycle step on an unknown context is `409 CONTEXT_NOT_FOUND`, as in `ml`. |
  | `405` | The route exists; the method does not. |
  | `500` | The service itself failed. The cause is in the log, not the body. |

- **Every response is JSON**, `Cache-Control: no-store`, and carries an
  `x-request-id` the service chose — the id on every log line about the
  request. An id a client sends is ignored.
- **Every call is safe to repeat.** The same mandate, cart and request key
  is the same purchase and gets the same context. A replayed payment,
  settlement, compensation or delivery gets the engine's idempotent answer,
  or its recorded refusal if the retry is a different thing. There is no
  idempotency store in the service: the ledger is one.
- **An unknown field is an error** (`422`), never a silently different
  request. Identifiers are at most 256 bytes, reasons at most 1024.

## Authentication

`Authorization: Bearer <key>`, with a key from `ML_API_KEYS`. A key is
full access: there is one tenant per deployment, and the key is the
operator's. The service keeps only SHA-256 digests of its keys and compares
a presented key against all of them in constant time; the log names a key
by the first eight hex characters of its digest. To rotate, list the old
and the new key together, move the clients, then drop the old one.
`/healthz`, `/readyz` and `/openapi.json` need no key.

## The routes

| Call | Body | Answer |
|---|---|---|
| `POST /v1/authorizations` | `mandate` (as the wallet signed it), `cart` (the native format), `request_key` | `200` the `Authorized` facts, `ctx` first |
| `POST /v1/contexts/{ctx}/payment` | `proof`: `reference`, `amount`, and for the sandbox rail `nonce`, `bound_cart`, `bound_merchant`, `unbound`, `valid` | `200` the `Paid` facts |
| `POST /v1/contexts/{ctx}/settlement` | `finality`: `reference`, `confirmations` or `failed`; empty to let the reference speak | `200` with `state` `settled`, `pending` or `settlement_failed` |
| `POST /v1/contexts/{ctx}/compensation` | `reference`, optional | `200` |
| `POST /v1/contexts/{ctx}/delivery` | `receipt`: `reference`, `signed_by` | `200` |
| `POST /v1/contexts/{ctx}/expiry` | none | `200` |
| `POST /v1/mandates/{mandate}/revocation` | none | `200` |
| `GET /v1/contexts/{ctx}` | | where it stands, or `404` |
| `GET /v1/contexts?mandate&state&after&limit` | | a page, with `next` |
| `GET /v1/events?after&limit` | | a page of the log, with `next` |
| `GET /v1/contexts/{ctx}/evidence` | | the bundle, signed by the service's key when it has one — what `ml verify` reads |
| `GET /healthz`, `GET /readyz` | | `{"status": "ok"}`, or `503 {"status": "not ready"}` |
| `GET /openapi.json` | | the document, generated from the code |

The full shapes are in [`crates/ml-serve/openapi.json`](../crates/ml-serve/openapi.json).
A lifecycle, with `jq` and a mandate from `ml mandate sign`:

```bash
H=(-H "Authorization: Bearer $KEY" -H 'Content-Type: application/json')
ctx=$(jq -n --slurpfile m mandate.json '{mandate: $m[0], request_key: "order-1",
      cart: {merchant: "bigbasket.com", total: {amount: "128.00", currency: "INR"}}}' \
    | curl -sS "${H[@]}" --data-binary @- localhost:8080/v1/authorizations | jq -r .ctx)
curl -sS "${H[@]}" -d '{"proof": {"reference": "pay-1-ok", "amount": {"amount": "128.00", "currency": "INR"}}}' \
    "localhost:8080/v1/contexts/$ctx/payment"
curl -sS "${H[@]}" -d '{"finality": {}}' "localhost:8080/v1/contexts/$ctx/settlement"
curl -sS "${H[@]}" -d '{"receipt": {"reference": "shipped"}}' "localhost:8080/v1/contexts/$ctx/delivery"
curl -sS "${H[@]}" "localhost:8080/v1/contexts/$ctx/evidence" > evidence.json && ml verify evidence.json
```

The rail is the sandbox rail until a real one lands: a payment reference
ending in `-ok` settles, `-fail` fails, `-reorg` gains a confirmation per
check and is then dropped, and `finality` can tell the rail what to report
instead. Its id defaults to `mock`, the same as `ml`'s, so the two agree
about one database.

## Configuration

Every variable is read at startup and every problem is reported at once.
A secret never appears in an error.

| Variable | Meaning | Default |
|---|---|---|
| `ML_LISTEN` | Address to bind | `127.0.0.1:8080` |
| `ML_DATABASE_URL` | PostgreSQL URL | required |
| `ML_DB_POOL` | Connections, 1–64; also the ledger calls in flight at once | `8` |
| `ML_API_KEYS` | Accepted keys, comma-separated, each at least 32 characters | required |
| `ML_TRUSTED_SIGNERS` | `principal=hex-public-key`, comma-separated | none: every mandate refused |
| `ML_MERCHANT_KEYS` | `key-id=hex-public-key`, comma-separated | none: only unsigned carts |
| `ML_RAIL_ID` | The rail's id, recorded with every payment | `mock` |
| `ML_RAIL_FINALITY` | Confirmations the rail needs, at least 1 | `1` |
| `ML_EVIDENCE_KEY` | Hex Ed25519 seed that signs exported evidence | none: bare bundles |
| `ML_REQUEST_TIMEOUT_SECS` | One ledger call, 1–300 | `30` |
| `ML_BODY_LIMIT` | Largest request body, bytes, 1 KiB–16 MiB | `262144` |
| `ML_SHUTDOWN_GRACE_SECS` | How long in-flight requests get after a stop signal, 0–600 | `20` |
| `ML_LOG` | A `tracing` filter | `info` |
| `ML_LOG_FORMAT` | `json` or `text` | `json` |

The public keys are the `public` field of a key file from `ml keys new`;
the evidence key is the `secret` field of one.

## Running it

- **Bind to loopback or a private network and put TLS in front.** The
  service speaks plain HTTP and expects a proxy: the proxy terminates TLS
  and owns the slow-client limits.
- **The store speaks to PostgreSQL without TLS.** Keep the database on the
  same host or a private network until the store grows a TLS pool
  (roadmap §8).
- **Load is shed, not queued.** Ledger calls run on blocking threads, one
  permit each, as many permits as the pool has connections; a request that
  finds none is `503` at once. A call that outlives
  `ML_REQUEST_TIMEOUT_SECS` is `503` too, but keeps running to its end —
  the permit is held until then, and whatever it recorded stands. One
  readiness check runs at a time, with a connection of its own, so nobody
  drains the pool through `/readyz`.
- **Logs** are JSON on stderr: method, path, status, latency, request id,
  key fingerprint, and for a refusal the context, stage and code. Never a
  body, a mandate, a cart or a key.
- **Stopping**: `SIGTERM` or Ctrl-C stops accepting, lets in-flight
  requests finish within `ML_SHUTDOWN_GRACE_SECS`, and exits. A bad
  configuration exits `64` before touching anything; a database that
  cannot be reached or prepared exits `1`.
- **Not yet**: metrics, per-key rate limits, the finality webhook, a
  Docker image. They are the next items in [`roadmap.md`](roadmap.md).

## Inside

The crate is a library with a thin binary. Handlers see the engine through
the `Backend` trait, so the tests drive the same routes over `MemoryStore`
and one test runs them over PostgreSQL. `openapi.json` is generated from
the handlers and the wire types; a test fails when the committed copy is
stale (`ML_WRITE_OPENAPI=1 cargo test -p ml-serve --test openapi`
regenerates it).
