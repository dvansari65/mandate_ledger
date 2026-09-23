# Roadmap

What it takes to get from here to a production-grade 0.1, in dependency
order. The idea every item serves: an enforcement layer an integrator puts
between an agent and a payment rail, which refuses any step where the
mandate, the cart, the proof and the finality disagree, and records why.

## 0.1 — "put this in front of an x402 payment in a sandbox"

### 1. Finish the harness
- [x] Read surface: `events_after`, `scan`, `last_seq`; the log in commit order
- [x] `ml` — keys, mandates, carts, `authorize`, `log`, `contexts`
- [x] `ml pay` / `settle` / `compensate` / `deliver` / `expire` / `revoke`
- [x] `ml evidence` / `ml verify` — verification with no database
- [ ] Controllable rail (`…-ok`, `…-fail`, `…-reorg`), `--rail`, and a test clock
- [ ] Eight scenario scripts, one per threat-model row, run by a CI job
      against the PostgreSQL service

### 2. Close the engine gaps found in review
- [ ] Payee binding: `VerifiedProof.bound_merchant`, `MERCHANT_BINDING_MISMATCH`
- [ ] B4: `authorize` replay compares the stored mandate — `CONTEXT_MISMATCH`
- [ ] B6: cap `Denied` events per context (the audit path is a storage DoS)
- [ ] Keyset paging for `scan`; `MemoryStore` pruning; the test suite's
      connection budget

### 3. Hygiene
- [ ] `SECURITY.md`, `CHANGELOG.md`, `#![deny(missing_docs)]` in the library
      crates, a stability policy for the refusal codes and wire formats,
      crates.io metadata, a release process, branch protection on `main`

### 4. The service and the SDK — the adoption path
- [ ] `ml serve`: the five calls over HTTP; status codes mirror the exit codes
      (200 allowed, 409 refused with the code, 503 could not decide); request
      keys; a finality webhook; health and readiness; structured logs and
      metrics; graceful shutdown; configuration from the environment; OpenAPI
- [ ] Docker image and `compose.yaml` with PostgreSQL
- [ ] A TypeScript SDK generated from the OpenAPI spec, with the refusal
      codes as a typed union, and a Node example
- Decisions to make first: auth model (API key or mTLS), threading behind a
  synchronous store, one ledger per deployment

### 5. The first real adapter: x402
- [ ] 402 terms → cart (`payTo` as the merchant, so §2's payee binding first)
- [ ] Signed payment payload → proof (its nonce, the terms hash, `payTo`)
- [ ] Chain receipt → finality; one rail id per network
- [ ] The eight scenarios through x402 payloads, plus spec test vectors
- Decisions: facilitator or direct chain verification; first network

## After 0.1

### 6. Money model
Partial captures, refunds as transitions under scope and velocity, currency
exponents, clock-skew tolerance, automatic expiry of lapsed authorizations.
Each adds a threat-model row and a scenario.

### 7. Trust
Delegation-chain verification (`parent` is written, never checked), key ids
and rotation with validity windows, a signed revocation list verifiable
offline, evidence redaction for third parties.

### 8. Operability
Retention for `ml_authorizations`, index review under load, backup and
restore guidance, pool sizing, published benchmarks, a multi-rail registry.

### 9. Assurance, before 1.0
Fuzzing the adapters, property tests that the PostgreSQL store is
indistinguishable from the in-memory one, model checking of the lifecycle,
an external audit. Nothing carries real money before the audit.

### 10. The remaining adapters
AP2, ACP, MPP — each proven by the same eight scenarios.
