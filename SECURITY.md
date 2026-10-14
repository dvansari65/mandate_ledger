# Security policy

mandate-ledger decides whether a payment step may proceed. A flaw in it is a
flaw in someone's money, so reports are handled privately until a fix is
out, and credited.

## Reporting

Use GitHub's private vulnerability reporting:
**<https://github.com/dvansari65/mandate_ledger/security/advisories/new>**.
Do not open a public issue or pull request for anything you believe is
exploitable.

A report is most useful when it names the guarantee it breaks — a row of
[`docs/threat-model.md`](docs/threat-model.md) or of the table in the
[README](README.md#what-it-guarantees) — and shows it: a failing script in
the style of [`scenarios/`](scenarios/), a failing test, or a sequence of
`ml` commands with their output.

You will have a reply within seven days. A confirmed vulnerability is fixed
on a private fork, released as a patch, and published as a GitHub security
advisory with an entry under *Security* in [`CHANGELOG.md`](CHANGELOG.md),
crediting you unless you prefer otherwise. Please give the fix a chance to
ship before disclosing.

## Scope

In scope is anything that defeats a guarantee the engine makes:

- a step allowed that the mandate, the cart, the proof or the finality
  should have refused, or a step recorded as something other than what
  happened;
- a budget, velocity, nonce or revocation check that does not hold under
  concurrency or across a restart, in `MemoryStore` or `PostgresStore`;
- an evidence bundle that verifies after it was altered, or a signature —
  mandate, merchant cart, exporter — that verifies when it should not;
- a bound in section B of the threat model that does not hold.

Out of scope, by design — section C of the threat model: prompt injection,
counterfeit storefronts, stolen agent or wallet keys, a merchant that does
not deliver after settlement, and attacks on the rail or the chain itself.
The `ml` command is a development tool; findings in it are welcome as
ordinary issues unless they reach the engine. The landing site is out of
scope.

## Supported versions

The project is pre-1.0. Fixes go into the next release and are not
backported; stay on the latest one.

## Dependencies

The dependency set is deliberately small. `cargo deny` checks licences,
advisories and sources, and `cargo audit` checks advisories, on every push
and pull request; Dependabot proposes updates weekly. Secret scanning and
push protection are on for the repository.
