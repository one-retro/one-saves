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
    # `multithread` is the only feature that is off by default, so --all-features covers the `Arc`
    # arm and nothing covers it standing on its own. This is the slim build a `Send` future takes.
    cargo test -p one-saves --no-default-features --features multithread
    cargo test -p one-saves-convert --no-default-features
    # `dat` without `dat-cmpro` refuses a ClrMamePro catalog in its own words, which is the one
    # feature-off arm that carries behaviour rather than just an absence.
    cargo test -p one-saves-convert --no-default-features --features dat
    cargo test -p one-saves-convert --no-default-features --features rom
    # `archive` on its own: an archive of loose saves can only be assembled by a format whose
    # single saves this reads, so without `ps1` the assembling arm is a different one.
    cargo test -p one-saves-convert --no-default-features --features archive
    cargo test -p one-saves-convert --no-default-features --features archive,ps1
    # One card format at a time. PS2 is the odd one — its saves are directories, so it is the only
    # format that does not use the shared save-to-part helper — and a lone `ps1` is the smallest
    # build that still has a card in it.
    cargo test -p one-saves-convert --no-default-features --features ps1
    cargo test -p one-saves-convert --no-default-features --features ps2
    cargo clippy -p one-saves-convert --all-targets --no-default-features --features n64
    cargo clippy -p one-saves-convert --all-targets --no-default-features --features gc
    cargo clippy -p one-saves-convert --all-targets --no-default-features --features vmu
    cargo test -p one-saves-convert --no-default-features --features neogeo
    # The default build, which `lint` does not reach: it runs --all-features, and what a user gets
    # by installing this is neither that nor --no-default-features. `icon` is the one off by
    # default that other code reads around, so this is the arm where an unread constant shows up.
    cargo clippy -p one-saves-convert --all-targets
    cargo clippy -p one-saves-cli --all-targets
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

# Install the git hooks from .cargo-husky/hooks (also installed by `just test`).
#
# cargo-husky installs from its own build script, which cargo caches, so an edit to a hook does
# not reinstall on its own. Cleaning it first is what makes this recipe mean what it says.
hooks:
    -cargo clean -q -p cargo-husky
    cargo build -q -p xtask --tests
    @echo "installed:" && ls .git/hooks/pre-commit .git/hooks/pre-push

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

# Cut a release: bump the version, run every gate, publish the workspace and tag it.
#
# `bump` is major, minor, patch or same. While the major is 0 the *minor* is Cargo's breaking
# position, so an additive change takes `patch` and a breaking one takes `minor`; `major` is the
# move to 1.0. `same` releases the number already in Cargo.toml, which is what a bump made by hand
# needs, and what a second run needs after one failed between the bump and the publish.
#
# Publishing cannot be undone: a version can be yanked but never reused. Everything that can fail
# is made to fail first — the gate, the oldest toolchain, and the packaging dry run all run before
# anything leaves the machine — and the tag is written last, so a tag means it went out.
# The list shows the last comment line above a recipe, which for this one is a caveat rather than
# a summary, so the summary is given outright.
[doc("Bump the version, run every gate, publish the workspace and tag it.")]
[confirm("This publishes every crate to crates.io, which cannot be undone. Continue?")]
release bump:
    #!/usr/bin/env bash
    set -euo pipefail

    # A published crate records the commit it came from, so that commit has to already be upstream
    # and there must be nothing local sitting on top of it.
    if [ -n "$(git status --porcelain)" ]; then
        echo "release: the working tree is dirty; commit or stash first" >&2
        exit 1
    fi
    branch=$(git rev-parse --abbrev-ref HEAD)
    if [ "$branch" != "main" ]; then
        echo "release: on $branch, and a release comes off main" >&2
        exit 1
    fi
    git fetch --quiet origin main
    if [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]; then
        echo "release: main and origin/main disagree; push or pull first" >&2
        exit 1
    fi

    version=$(cargo xtask version {{ bump }})
    tag="v$version"
    if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
        echo "release: $tag already exists, so $version has been released from here before" >&2
        exit 1
    fi

    # Everything CI runs, plus the oldest toolchain it claims to support. The bump is in the tree
    # by now, so this is the gate running against the version that will actually go out.
    just check
    just msrv

    # `cargo publish` refuses a dirty tree, so the bump is committed before the dry run rather
    # than after it. `same` moved nothing and has nothing to commit.
    if ! git diff --quiet; then
        git add Cargo.toml Cargo.lock
        git commit --message "Release $version"
    fi

    just package
    git push origin main
    cargo publish --workspace

    # Last, and only on success: a tag that exists is a release that happened.
    git tag --annotate "$tag" --message "Release $version"
    git push origin "$tag"
    echo "released $version"

# Remove build artifacts.
clean:
    cargo clean
