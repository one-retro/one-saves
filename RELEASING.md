# Releasing

## Publish order

The crates depend on each other by path *and* version, so they have to go to crates.io in
dependency order. A crate cannot be packaged until everything it depends on is already published:
`cargo package` resolves path dependencies against the registry, not against the workspace.

```
1. one-saves            (dcbor, sha2, zstd)
2. one-saves-registry   (nothing)
3. ps2-memcard          (nothing)
4. one-saves-convert    (1, 2, 3)
5. one-saves-cli        (1, 2, 4)
```

`one-saves-registry` and `ps2-memcard` depend on nothing in this workspace and can go at any point
before step 4.

```console
for crate in one-saves one-saves-registry ps2-memcard one-saves-convert one-saves-cli; do
  cargo publish -p "$crate"
  # Wait for the index to catch up before the next one, or its path dependency will not resolve.
done
```

## Before publishing

```console
just check      # fmt, lint, tests, docs, and the registry drift check
just msrv       # the declared minimum Rust version
just package    # dry-run packaging, in dependency order
```

CI runs everything `just check` does. `just msrv` is checked there too; `just package` is not,
since the last two crates cannot be packaged until their dependencies are published. It also regenerates `crates/one-saves-registry/src/generated.rs` from the
committed JSON and fails on a diff, so a hand edit to either half shows up rather than diverging
quietly.

## Version numbers

Every crate shares the workspace version. The specification is at 0.1 and **not yet stabilized**,
so until it reaches 1.0 these stay on 0.x and breaking changes happen in place.

`one-saves::SPEC_VERSION` and the vendored corpus's `manifest.json` must agree; the conformance
suite asserts it, so a corpus re-vendored from a newer spec fails the build until the crate is
updated to match.

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
