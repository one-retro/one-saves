# one-saves

The [Universal Saves Format][spec] (`.1saves`, `1SAV`): a portable, self-describing CBOR container
for retro console save files and everything attached to them.

```rust,no_run
use one_saves::Bundle;

let bundle = Bundle::from_slice(&std::fs::read("card.1saves")?)?;
for part in &bundle.parts {
    println!("part {} is {} bytes", part.id, part.payload.len());
}
println!("content hash: {}", bundle.content_hash()?);
```

## What it does

- **Decodes and encodes** bundles, enforcing the deterministic encoding the format rests on.
- **Validates** every structural rule: nesting depth, normalized nested bundles, part addressing,
  hash-value ordering, and the shape rules that make `size` present exactly where it is needed.
- **Hashes** a bundle two ways. The *file hash* identifies this exact file; the *content hash* is
  a pure function of what the bundle says, and is what a content-addressable store keys on.
- **Round-trips what it does not understand**: an extension key from a producer you have never
  heard of, and an integer key from a later minor version, both survive unchanged.

## Two strictness modes

`Strictness::Decoder` is what to ship: an unrecognised integer key is kept and round-tripped,
because the only thing it can be is a field a later minor version assigned.

`Strictness::Schema` is the v0.1 schema exactly, which rejects such a key. It is what the
conformance corpus tests, and it is not what a shipped decoder should do.

## Conformance

Passes all 73 cases of the corpus published with the specification, plus direct tests for the four
properties the corpus README says a green run cannot prove. See the [workspace README][root].

## Features

- `zstd` (default) — read and write parts whose payload carries `encoding: "zstd"`. Without it,
  such a part still decodes and round-trips; its payload just cannot be inflated.

[spec]: https://docs.1retro.com/specifications/universal-saves-format/
[root]: https://github.com/one-retro/one-saves
