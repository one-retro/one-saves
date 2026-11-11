# one-saves

The [Universal Saves Format][spec] (`.1saves`, `1SAV`): a portable, self-describing CBOR container
for retro console save files and everything attached to them, an emulator's save states and
screenshots among it.

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
- **Validates** every structural rule, at every depth: what each shape may hold, normalized nested
  bundles, part addressing, hash-value ordering, and the rules that make `size` present exactly
  where it is needed. One malformed save fails the card it sits on, since what a decoder salvaged
  would be a bundle nobody wrote.
- **Reads a bundle's shape** — a save, a save state, a screenshot, a card, a device or a
  collection — and holds its header and its parts to that shape's rules, so every consumer reads
  the same file the same way. A state names the core it loads into, holds exactly one `state` part
  and at most one screenshot, and says when and with what it was taken.
- **Reads a bundle as it arrives**, with `stream::Reader`: a push parser that judges the header
  before any part has arrived and each part before its payload, hands each payload on as it goes
  by, and holds no more than the limits it is given, whatever the input. It accepts exactly what
  `Bundle::from_slice` accepts with every part's digest checked, refuses the rest with the same
  errors, and does no I/O of its own, so it runs under an async runtime without depending on one.
- **Hashes** a bundle two ways. The *file hash* identifies this exact file; the *content hash* is
  a pure function of what the bundle says, and is what a content-addressable store keys on.
- **Types the extension keys the specification defines** — a clock reading, a label, an icon, a
  directory's dates — so a caller asks `extensions.get::<Label>()` rather than walking CBOR. A
  producer's own key is a type implementing the same `Extension` trait.
- **Round-trips what it does not understand**: an extension key from a producer you have never
  heard of, and an integer key from a later minor version, both survive unchanged. Decoding and
  re-encoding gives back the bytes it was handed, for every file it accepts.

## Two strictness modes

`Strictness::Decoder` is what to ship: an unrecognised integer key is kept and round-tripped,
because the only thing it can be is a field a later minor version assigned.

`Strictness::Schema` is the schema of the version this crate implements, exactly, which rejects
such a key. It is what the conformance corpus tests, and it is not what a shipped decoder should
do.

## Conformance

Passes all 129 cases of the corpus published with the specification, plus direct tests for the four
properties the corpus README says a green run cannot prove. See the [workspace README][root].

## Features

- `zstd` (default) — read and write parts whose payload carries `encoding: "zstd"`. Without it,
  such a part still decodes and round-trips; its payload just cannot be inflated.
- `multithread` (off) — `Arc` rather than `Rc` under `dcbor`, which is what makes `Bundle` and
  everything reachable from it `Send` and `Sync`. Turn it on to hold a bundle across an `.await`
  in a future that has to be `Send`. It changes no behaviour and no encoding; the cost is atomic
  reference counting, which a caller that stays on one thread has no use for.

[spec]: https://docs.1retro.com/specifications/universal-saves-format/
[root]: https://github.com/one-retro/one-saves
