# Task runner for this workspace: https://just.systems
#
# This file is what CI runs: every job in .github/workflows/ci.yml is a `just` recipe and nothing
# else, so `just check` locally and a green run there are the same commands by construction. Change
# what CI does by changing a recipe; touch the workflow only to add a job or a toolchain component.
#
# `just check` is every recipe CI runs, in the order it runs them. A recipe added here belongs in
# `check` — and, if it needs a job of its own rather than a step, in the workflow too.

# The Rust version the workspace declares. `dcbor` uses let-chains and states no rust-version of
# its own, so ours has to state the real floor.
msrv := "1.88"

# Where a checkout of the specifications repository lives, for syncing the registries.
docs := "../docs.1retro.com"

# List the recipes.
default:
    @just --list

# Everything CI runs.
check: fmt-check lint test test-features doc registries-in-sync

# Run the test suite.
test:
    cargo test --workspace --all-features

# Run the conformance corpus on its own, with output.
conformance:
    cargo test -p one-saves --test conformance -- --nocapture

# Run the converters against the real emulator saves under data/.
fixtures:
    cargo test -p one-saves-convert --test real_saves -- --nocapture

# `just test` runs --all-features, which never compiles a `#[cfg(not(feature = ...))]` arm, so a
# slim build can rot with CI green. Clippy is in here for the same reason: the lints that fire on
# an arm only one configuration has are lints --all-features cannot see.

# Check every crate still builds and passes with its features turned down.
test-features:
    cargo test -p one-saves --no-default-features
    cargo test -p one-saves-convert --no-default-features
    # `dat` without `dat-cmpro` refuses a ClrMamePro catalog in its own words, which is the one
    # feature-off arm that carries behaviour rather than just an absence.
    cargo test -p one-saves-convert --no-default-features --features dat
    cargo test -p one-saves-convert --no-default-features --features rom
    # One card format at a time. PS2 is the odd one — its saves are directories, so it is the only
    # format that does not use the shared save-to-part helper — and a lone `ps1` is the smallest
    # build that still has a card in it.
    cargo test -p one-saves-convert --no-default-features --features ps1
    cargo test -p one-saves-convert --no-default-features --features ps2
    cargo clippy -p one-saves-convert --all-targets --no-default-features --features n64
    cargo clippy -p one-saves-convert --all-targets --no-default-features --features gc
    cargo clippy -p one-saves-convert --all-targets --no-default-features --features vmu
    cargo test -p one-saves-convert --no-default-features --features neogeo
    # The CLI has no tests; what these check is that the flag-free and card-free arms compile clean.
    cargo clippy -p one-saves-cli --all-targets --no-default-features
    cargo clippy -p one-saves-cli --all-targets --no-default-features --features rom
    cargo clippy -p one-saves-cli --all-targets --no-default-features --features cards

# Format the tree.
fmt:
    cargo fmt --all

# Check formatting without changing anything.
fmt-check:
    cargo fmt --all --check

# Lint everything, tests and examples included.
lint:
    cargo clippy --workspace --all-targets --all-features

# Apply the lints clippy can fix on its own.
lint-fix:
    cargo clippy --workspace --all-targets --all-features --fix --allow-dirty

# Build the documentation.
doc:
    cargo doc --workspace --no-deps

# Build the documentation and open it.
doc-open:
    cargo doc --workspace --no-deps --open

# Check the workspace still builds on the oldest Rust it claims to support.
msrv:
    rustup toolchain install {{ msrv }} --profile minimal 2>/dev/null || true
    cargo +{{ msrv }} check --workspace --all-features

# Rebuild the registry tables from the JSON already in the repository.
registries:
    cargo xtask registries

# Re-read the registries from the specification pages, then rebuild the tables.
sync-registries docs=docs:
    cargo xtask registries --docs {{ docs }}

# Fail if the committed tables have drifted from their data. This is what CI checks.
registries-in-sync: registries
    git diff --exit-code crates/one-saves-registry/src/generated.rs

# Re-vendor the conformance corpus from a checkout of the specifications.
vendor-conformance docs=docs:
    # The corpus is generated there and pinned to the spec_version it records, so this is the only
    # way it should ever change; the suite checks a digest per case and will catch a hand edit.
    rm -rf crates/one-saves/tests/conformance
    cp -R {{ docs }}/conformance/universal-saves crates/one-saves/tests/conformance
    cargo test -p one-saves --test conformance

# Install the `1saves` binary from this checkout.
install:
    cargo install --path crates/one-saves-cli --locked

# Dry-run the whole release: package every crate and verify the dependents against it.
package:
    # Everything publishing does except the upload. Cargo packages each crate, then builds the
    # dependent ones against those packaged copies rather than against the workspace, which is
    # what catches a file the tests reach for but `cargo package` leaves out.
    cargo publish --workspace --dry-run

# Remove build artifacts.
clean:
    cargo clean
