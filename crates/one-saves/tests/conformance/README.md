# Universal Saves Format conformance cases

Bundles for testing an implementation of the
[Universal Saves Format](https://docs.1retro.com/specifications/universal-saves-format/). The layout, the manifest
fields and how to run a corpus are in the [parent README](../README.md); this page covers what is specific to this spec.

Cases are generated from `test/bundles.js`.

## What a green run does not cover

**Determinism.** Two encoders given the same logical bundle must produce the same bytes, which no single file can
demonstrate. Test it directly: decode each `valid` case, re-encode it, and assert the bytes are identical. That
round-trip is what the
[content hash](https://docs.1retro.com/specifications/universal-saves-format/#content-hash-and-file-hash) depends on,
and it is the property most likely to be quietly wrong in a new implementation.

**Nesting depth.** The corpus carries a `collection` case reaching the depth-2 cap, but nothing beyond it, since a
bundle deeper than the cap is rejected by a rule about structure rather than by anything a decoder sees in one map.
Build an over-deep bundle in your own tests and check that it is refused.

**Whether an unknown value round-trips.** Several `valid` cases carry a name or key the spec expects a decoder not to
recognise: `third-party-kind`, `part-extension-keys`, `game-ids` with an unknown resolver. Accepting them is only half
the requirement. The other half is that re-encoding preserves them unchanged, which again is a round-trip test rather
than an accept/reject one.

## Schemas

The CDDL schemas are not vendored here, since an implementation checks these rules natively rather than by shelling out
to a validator. They live beside the specifications:

- [`universal-saves-format.cddl`](https://github.com/hansl/docs.1retro.com/blob/main/src/content/docs/specifications/universal-saves-format.cddl)
  for the bundle itself
- one file per extension key under
  [`specifications/extensions/`](https://github.com/hansl/docs.1retro.com/tree/main/src/content/docs/specifications/extensions)

A schema describes one version exactly and rejects integer keys it does not list. A shipped decoder is deliberately
looser: it ignores and round-trips an integer key it does not know, because the only thing such a key can be is a later
minor version's field. Do not derive decoder behaviour from the schema alone.
