# Stability

What an integrator may rely on from one version to the next, what may
change, and how a change is announced. The project is 0.x and the Rust API
is still moving. The things that outlive a process — refusal codes in a
dashboard, bundles in a dispute file, rows in a database, exit codes in a
script — are governed now, because changing them later would cost the
people who adopted early.

## Versions

`ml-core`, `ml-adapters` and `ml-store-postgres` share one version and are
released together. Cargo's reading of semantic versioning applies: while
the version is `0.0.z`, every release may break the Rust API; from `0.y.0`
on, a change of `y` may and a change of `z` may not. The commitments below
hold either way. A release may change a function signature; it may not
change what a refusal code means or how an event is hashed.

## Codes and names

These strings are the contract for scripts, SDKs and dashboards:

- the 26 refusal codes, `DenyReason::code()` — `SCOPE_MERCHANT_MISMATCH`,
  `NONCE_ALREADY_USED` and the rest;
- the 8 evidence verification codes, `EvidenceError::code()` —
  `HASH_MISMATCH`, `BROKEN_CHAIN` and the rest;
- the names of the states — `authorized`, `paid`, `settled`,
  `settlement_failed`, `compensated`, `delivered`, `expired` — of the
  stages, and of the event types, which are the states plus `denied`.

A code is never renamed, reused or given a different meaning. Its meaning
is pinned by a row of [`threat-model.md`](threat-model.md) and a test in
`crates/ml-verify/tests/denials.rs`. A code that no step produces any longer
stays defined, documented as retired, and `DenyReason::from_code` keeps
parsing it.

Codes are added. `DenyReason` is `#[non_exhaustive]` for this reason, and a
consumer must treat a code it does not know as what it is: a refusal. Never
map an unknown code to success, and never retry on it. The rule in
[`integration.md`](integration.md) — a `Denied` is final, an operational
error is retried — does not depend on recognising the code.

`detail` is prose for people. Its wording may change in any release. Do not
parse it.

## The evidence format

An `EvidenceBundle` is JSON: `version`, `ctx`, `generated_at` and `events`,
oldest first. A `SignedEvidence` wraps one with the exporter's public key
and signature. Each event carries `seq`, `ctx`, `at`, `prev_hash`, `hash`
and a `body` whose `type` is the event type. The encodings: a hash is
`sha256:` followed by 64 hex characters; keys and signatures are hex; the
raw cart bytes in an `authorized` event are base64; an amount is an object
of a decimal string and a currency code; a timestamp is integer Unix
seconds; an identifier is a string.

An event's hash is SHA-256 over the previous hash followed by the RFC 8785
canonical JSON of its `seq`, `ctx`, `at` and `body`. A mandate's signature
is over the canonical JSON of its body. So the canonical form of every
event body, of `MandateBody` and of `CartSnapshot` is part of the format,
and three rules keep every bundle ever exported verifiable:

1. A field is never removed or renamed, and its encoding never changes.
2. A field added to anything hashed or signed is optional and omitted when
   absent, so that a value written before the field existed canonicalises
   to the same bytes after it. Fields that today serialise as `null` when
   absent keep doing so, for the same reason.
3. What the first two rules cannot accommodate is a new `EVIDENCE_VERSION`.
   A verifier refuses a version it does not know — `UNSUPPORTED_VERSION` —
   rather than apply the wrong rules, and a new version does not retire
   the old: a build verifies every version it has ever produced.

`ml verify` is the reference: a bundle it verified once, it verifies forever.

## The database schema

The PostgreSQL schema is a sequence of numbered migrations. One that has
shipped in a release is never edited; a change is a new file. `migrate()`
applies the ones a database lacks, and an older build refuses a database
that is ahead of it rather than run against tables it does not understand.
Existing columns keep their names and meaning; columns and indexes are
added. The `body` column of `ml_events` holds an event body exactly as it
appears in a bundle, so the rules above apply to it.

## The `ml` command

Exit codes `0`, `1`, `2` and `64` mean what [`sandbox.md`](sandbox.md) says
they mean. Under `--json`, existing keys keep their names and types; keys
are added, not removed. The human-readable output is not a contract. `ml`
is not published to crates.io; the scripts in `scenarios/` are its contract
test.

## The service

The status codes of `ml-serve` mean what [`service.md`](service.md) says
they mean, and a refusal body carries the same `code`, `stage`, `detail`,
`ctx` and `recorded` as the `ml` command's report. Existing paths and keys
keep their names and types; keys are added, not removed. Request bodies
refuse unknown fields, so a new field is never ignored by an older
service: a client sends what the version it targets accepts.

## The Rust API

Semantic versioning, as read above. Beyond that:

- The traits an integrator implements — `CartAdapter`, `Rail`,
  `SignerPolicy`, `Clock`, `Store` — gain a required method only in a
  release that may break. `Store` gains a method with a default body when
  a correct default exists, and a default that is only adequate for a
  small store says so in its documentation.
- `DenyReason` is `#[non_exhaustive]`. Until 1.0, match the other public
  enums with a wildcard arm too; they will gain variants.
- The minimum supported Rust version, 1.85, is raised only in a release
  that may break, never in a patch.
- A deprecated item keeps working for at least one such release before it
  is removed, and its `#[deprecated]` attribute names the replacement.
- Not a contract, in any release: `Debug` and `Display` output, the text of
  error messages, the values of tunables such as `DENIAL_CAP`, and which
  advisory lock keys the PostgreSQL store takes.

## How a change is announced

Every release has an entry in [`CHANGELOG.md`](../CHANGELOG.md) whose
heading says whether it may break the Rust API. An entry that touches a
code, a format or the schema says so in its first words, and a format
change ships in the same release as the verifier for the old version.
