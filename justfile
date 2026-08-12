# Task runner for this workspace: https://just.systems
#
# `just check` runs what CI runs, in the order CI runs it, so a green run here means a green run
# there. Anything added to .github/workflows/ci.yml belongs in that recipe too.

# The Rust version the workspace declares. `dcbor` uses let-chains and states no rust-version of
# its own, so ours has to state the real floor.
msrv := "1.88"

# Where a checkout of the specifications repository lives, for syncing the registries.
docs := "../docs.1retro.com"

# List the recipes.
default:
    @just --list

# Everything CI runs.
check: fmt-check lint test test-no-default doc registries-in-sync

# Run the test suite.
test:
    cargo test --workspace --all-features

# Run the conformance corpus on its own, with output.
conformance:
    cargo test -p one-saves --test conformance -- --nocapture

# Run the converters against the real emulator saves under data/.
fixtures:
    cargo test -p one-saves-convert --test real_saves -- --nocapture

# Check the format crate still builds and passes without zstd.
test-no-default:
    cargo test -p one-saves --no-default-features

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

# Dry-run packaging every publishable crate, in dependency order.
package:
    #!/usr/bin/env bash
    # The last two fail until their dependencies are on crates.io, since `cargo package` resolves
    # path dependencies against the registry. See RELEASING.md.
    for crate in one-saves one-saves-registry ps2-memcard one-saves-convert one-saves-cli; do
        printf '%-20s' "$crate"
        cargo package -p "$crate" --no-verify --allow-dirty 2>&1 | tail -1
    done

# Remove build artifacts.
clean:
    cargo clean
