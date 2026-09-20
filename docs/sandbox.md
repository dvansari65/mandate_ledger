# Sandbox

The `ml` command drives the engine from a terminal. Every command is one
process, so a lifecycle that spans commands is a lifecycle that survived a
restart. This page is the operator's guide; it grows with the command.

```bash
cargo install --path crates/ml-sandbox     # or: cargo run -q -p ml-sandbox -- <args>
```

## Conventions

Every command follows these, and scripts rely on them.

- **State is files and the database, nothing else.** Keys, mandates and carts
  are JSON files you can read and edit. Nothing is remembered between two
  commands except what the store holds.
- **`--json`** prints one JSON object on stdout in place of the human-readable
  `label  value` lines, with the same keys. Errors always go to stderr,
  prefixed `error:`.
- **Exit codes mean something.**

  | Code | Meaning |
  |---|---|
  | `0` | The step was allowed, or the command had nothing to decide. |
  | `2` | The ledger evaluated the step and **refused** it. A decision, not a failure; the reason code is on stderr. |
  | `1` | The ledger could not decide: a file, the store or the rail failed. Fix the cause and retry. |
  | `64` | The command line was wrong. Distinct from a refusal on purpose. |

## Keys

```bash
ml keys new --out user.key
```

Writes an Ed25519 key pair as JSON — `algorithm`, `public`, `secret`, hex —
readable by its owner alone on Unix, and never overwrites an existing file. The
secret is in the clear: this is a sandbox key. Make one for the principal
(the wallet) and one for each merchant.

## Mandates

```bash
ml mandate sign body.json --key user.key --out mandate.json
```

`body.json` is the mandate body exactly as the engine signs it. Nothing is
filled in for you: a mandate is a statement of authority, and every field of
it should be deliberate.

```json
{
  "id": "mnd-1",
  "principal": "user:alice",
  "agent": "agent:shopper",
  "scope": {
    "merchants": ["bigbasket.com", "*.zepto.com"],
    "categories": ["grocery"],
    "currency": "INR",
    "max_per_txn": { "amount": "2000.00", "currency": "INR" },
    "max_total": { "amount": "8000.00", "currency": "INR" },
    "valid_from": 1800000000,
    "valid_until": 1802592000,
    "velocity": { "max_count": 5, "window_secs": 86400 },
    "min_attestation": "merchant_signed"
  },
  "issued_at": 1800000000
}
```

`categories`, `max_per_txn`, `max_total`, `velocity` and `parent` may be
omitted; `merchants` accepts `"*"`, an exact id, or `"*.suffix"`; times are
Unix seconds; `min_attestation` is `agent_reported` or `merchant_signed`.
The output is the signed mandate the engine accepts — the body plus the
signer's public key and the signature, as hex. An invalid scope (a window
that ends before it starts, a cap in another currency) is an error, not a
mandate.
