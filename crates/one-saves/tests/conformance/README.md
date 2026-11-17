# Universal Saves Format conformance cases

Bundles for testing an implementation of the
[Universal Saves Format](https://docs.1retro.com/specifications/universal-saves-format/). The layout, the manifest
fields and how to run a corpus are in the [parent README](../README.md); this page covers what is specific to this spec.

Cases are generated from `test/bundles.js`.

## What a green run does not cover

**Determinism.** Two encoders given the same logical bundle must produce the same bytes, which no single file can
demonstrate. Test it directly: decode each `valid` case, re-encode it, and assert the bytes are identical. That
round-trip is what the [content hash](https://docs.1retro.com/specifications/bundle/#content-hash) depends on, and it is
the property most likely to be quietly wrong in a new implementation.

**Nesting depth.** No depth is written down: what a bundle may hold follows from its shape, and the chain of shapes ends
on its own. `collection-of-devices` reaches four levels, and the `invalid` cases cover each shape holding one it may
not. An implementation that caps the depth at some number of its own will reject legal bundles.

**Whether an unknown value round-trips.** The `round-trip` cases, and several `valid` ones, carry a name or key the spec
expects a decoder not to recognise: `third-party-kind`, `part-extension-keys`, `game-extension-keys`, `game-ids` with an
unknown resolver, `n64-chip-with-a-later-key`. Accepting them is only half the requirement. The other half is that
re-encoding preserves them unchanged, which is a round-trip test rather than an accept/reject one.

## Schemas

The CDDL schemas are not vendored here, since an implementation checks these rules natively rather than by shelling out
to a validator. They live beside the specifications:

- [`universal-saves-format.cddl`](https://github.com/one-retro/docs/blob/main/src/content/docs/specifications/universal-saves-format.cddl)
  for the bundle itself
- one file per extension key under
  [`specifications/extensions/`](https://github.com/one-retro/docs/tree/main/src/content/docs/specifications/extensions)

A schema describes one version exactly. It rejects integer keys it does not list, and names it does not list in a
vocabulary the spec owns. A shipped decoder is deliberately looser: it round-trips both, because the only thing either
can be is a later version's. The `round-trip` cases are those. Do not derive decoder behaviour from the schema alone.
