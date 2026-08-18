# Releasing

## Cutting one

```console
just release patch        # major, minor, patch or same
```

That is the whole of it: the recipe bumps the version, runs the gate, commits, pushes, publishes
the workspace and tags it. It refuses to start unless the working tree is clean, the branch is
`main`, and `main` and `origin/main` are the same commit — a published crate records the commit it
came from, so that commit has to already be fetchable.

Everything that can fail is made to fail first. `just check`, `just msrv` and the packaging dry run
all run before anything leaves the machine, and the tag is written last, after the upload, so a tag
that exists is a release that happened.

Which word to pass: while the major is 0 the **minor** is Cargo's breaking position, so an additive
change takes `patch` and a breaking one takes `minor`. `major` is the move to 1.0, which for this
workspace means the specification stabilized. `same` releases the number already in `Cargo.toml` —
for a bump made by hand, and for a second run after one failed between the bump and the publish,
which is the one failure that leaves a commit behind without a release.

The rest of this file is what that recipe does, for when it has to be done by hand.

## The version number

Every crate shares the workspace version: each member carries `version.workspace = true`, so there
is no per-crate number to edit. What there is instead is the same number written ten times in
the root `Cargo.toml` — once under `[workspace.package]`, and once in each member's entry under
`[workspace.dependencies]`, where the pin is what makes a published crate depend on the version it
was released beside rather than on whatever is newest.

```console
cargo xtask version patch     # writes all of them, and prints the new number
```

A pin left behind is not caught by the build, because a path dependency resolves against the
workspace either way. It surfaces at `cargo publish`, which resolves against the registry — the
worst moment to find out. The task refuses outright if the copies disagree before it starts, since
that means an earlier edit went half-done and which half was right is not something to guess.

## Publish order

The crates depend on each other by path *and* version, so they have to go to crates.io in
dependency order. A crate cannot be packaged until everything it depends on is already published:
`cargo package` resolves path dependencies against the registry, not against the workspace.

```
1. one-saves            (dcbor, sha2, zstd)
2. one-saves-registry   (nothing)
3. dreamcast-vmu        (nothing)
   gc-memcard           (nothing)
   n64-cpak             (nothing)
   neogeo-memcard       (nothing)
   ps1-memcard          (nothing)
   ps2-memcard          (nothing)
4. one-saves-convert    (1, 2, 3)
5. one-saves-cli        (1, 2, 4)
```

`one-saves-registry` and the six card crates depend on nothing at all — not on this workspace, not
on anything outside it — so they can go at any point before step 4, in any order.

Cargo works that order out itself, so the whole workspace goes in one command. It publishes in
dependency order and waits for each crate to appear in the index before the next one needs it:

```console
cargo publish --workspace
```

`xtask` is not published; it carries `publish = false`.

On cargo older than 1.90, `--workspace` is not available and the crates go one at a time, in the
order above, pausing between each for the index to catch up:

```console
for crate in one-saves one-saves-registry dreamcast-vmu gc-memcard n64-cpak neogeo-memcard \
             ps1-memcard ps2-memcard one-saves-convert one-saves-cli; do
  cargo publish -p "$crate"
done
```

## Before publishing

```console
cargo xtask version patch         # every copy of the version, in one file
git push                          # a published crate records its commit; make it fetchable first
just check                        # fmt, lint, tests, docs, and the registry drift check
just msrv                         # the declared minimum Rust version
just package                      # cargo publish --workspace --dry-run
```

Commit the bump before the dry run rather than after it: `cargo publish` refuses to package a
dirty tree, in either mode.

The dry run does everything but upload: it packages each crate, then builds the dependent ones
against the packaged copies rather than against the workspace, which is what catches a file that
the tests reach for but `cargo package` leaves out.

Publishing cannot be undone. A version can be yanked, which stops new dependants resolving it, but
it stays downloadable and its number can never be reused — and the crate name is claimed for good.

CI runs everything `just check` does, including `just msrv`. It does not run the dry run, which
needs a network fetch per crate. `just check` also regenerates
`crates/one-saves-registry/src/generated.rs` from the committed JSON and fails on a diff, so a hand
edit to either half shows up rather than diverging quietly.

## Version policy

The specification is at 0.1 and **not yet stabilized**, so until it reaches 1.0 these stay on 0.x
and breaking changes happen in place.

`one-saves::SPEC_VERSION` and the vendored corpus's `manifest.json` must agree; the conformance
suite asserts it, so a corpus re-vendored from a newer spec fails the build until the crate is
updated to match.

Releases are tagged `v<version>`, on the commit that carries the bump.

## Re-vendoring the conformance corpus

The corpus under `crates/one-saves/tests/conformance/` is copied from the specifications
repository. When that spec moves:

```console
just vendor-conformance       # defaults to ../docs.1retro.com
```

Do not edit anything under that directory by hand. The manifest carries a digest per case, and the
harness checks it, so a hand edit shows up as corruption rather than as a passing test.

## Syncing the registries

The registry data is generated from the registry pages at docs.1retro.com:

```console
just sync-registries          # defaults to ../docs.1retro.com
just sync-registries ../elsewhere
```

Without `--docs` the task rebuilds the tables from the JSON this repository already carries, which
is what CI runs: it checks the committed tables still match their data and needs nothing fetched.

Neither form runs at build time. The JSON is kept as well as the generated Rust because it is what
makes a registry change reviewable — a diff of the tables is a diff of the data, not of a wall of
static initialisers.
