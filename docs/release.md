# Releasing

How a version of mandate-ledger is cut and published. The three crates —
`ml-core`, `ml-adapters`, `ml-store-postgres` — share one version and ship
together. `ml` is built from the repository and is not published.

## Before

- `main` is green, and *Unreleased* in `CHANGELOG.md` has everything since
  the last release. Open security reports are fixed, or deferred knowingly.
- Decide the version with [`stability.md`](stability.md): on `0.0.z` any
  release may break the Rust API; from `0.y.0`, `y` for a breaking release
  and `z` otherwise.
- `cargo login` with a crates.io account that owns the three crates. The
  first publish of a name claims it; after that, publishing needs an owner.
  The token lives in Cargo's credential store, never in the repository, and
  CI does not publish.
- Cargo 1.90 or newer, for `cargo publish --workspace`. The MSRV, 1.85, is
  for people building the crates, not for releasing them.

## Cut it

On a branch off `main`:

1. Bump the version in `Cargo.toml`: `[workspace.package].version` and the
   three `version =` fields under `[workspace.dependencies]`, which path
   dependencies need for publishing. `grep -n 'version = "' Cargo.toml`
   shows all four. Then `cargo update --workspace`, so the lockfile follows.
2. In `CHANGELOG.md`, rename *Unreleased* to `[x.y.z] — YYYY-MM-DD`, add a
   fresh empty *Unreleased* above it, and update the link definitions at
   the bottom.
3. Update the version in the README's status section.
4. Run the whole release short of the upload:

   ```bash
   cargo +stable publish --workspace --dry-run
   ```

   It packages the three crates, builds each one from its package with the
   others resolved from the same set, and stops at the upload. Fix anything
   it finds before going on.
5. Open the pull request, let CI finish, merge.

## Ship it

On `main` at the merge commit:

```bash
git switch main && git pull --ff-only
git tag -s v0.0.1 -m "mandate-ledger 0.0.1"
git push origin v0.0.1
cargo +stable publish --workspace
```

`publish --workspace` uploads `ml-core`, then `ml-adapters` and
`ml-store-postgres` once the index has it, and skips the crates marked
`publish = false`. If it stops part way, publish the ones that did not go,
one at a time in that order, with `cargo +stable publish -p <crate>`.

Then create a GitHub release from the tag with the changelog entry as its
notes, and check that docs.rs has built all three crates.

## If it went wrong

A published version cannot be changed or deleted. Yank it — `cargo yank
--version x.y.z ml-core`, and the others — so no new project resolves to
it, and release the fix as the next patch. Never reuse a version number,
and never move a tag.

A security fix follows the same steps; the advisory is published once the
release is out.
