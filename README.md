# one-saves

Rust implementation of the [Universal Saves Format][spec] (`.1saves`, `1SAV`): a portable,
self-describing CBOR container for retro console save files and everything attached to them.

A retro save is a bare binary dump with no agreed way to say what game it belongs to, what wrote
it, or what else belongs with it. Every emulator invented its own convention. This is the
container that fixes that, plus the tools to move real saves and memory cards in and out of it.

```console
$ 1saves convert card.mcr --from duckstation
card.mcr -> card.1saves (PS1 memory card, 3 save(s), 25525 bytes)

$ 1saves inspect card.1saves
  system       psx (PlayStation)
  card         ps1-mc, capacity 131072 bytes
  parts        3
    [0]   bundle     memcard-1             8264 bytes  2db9dc7f051620d4  SCUS-94163   slot 1   BASCUS-94163FF7-S01
    [1]   bundle     memcard-1             8264 bytes  1364dd5cfc4c8b54  SCUS-94163   slot 2   BASCUS-94163FF7-S03
    [2]   bundle     memcard-1             8264 bytes  60fff59a7d0ca245  SLUS-00594   slot 3   BASLUS-00594
```

Each part line is its id, kind, socket, size, the head of its digest, the game's product code, the
slot it sat in and the name the card held. The name goes last because it is the one field with no
useful bound — a GameCube filename runs to 32 characters — so everything before it stays in column.
The two Final Fantasy VII saves share a product code and are told apart by the rest.

## The crates

| Crate | What it is |
| ----- | ---------- |
| [`one-saves`](crates/one-saves) | The format: decode, encode, validate, content hash. |
| [`one-saves-registry`](crates/one-saves-registry) | Systems, emulator cores, save roles, vendor names. |
| [`one-saves-convert`](crates/one-saves-convert) | Emulator saves and memory cards, both directions. |
| [`one-saves-cli`](crates/one-saves-cli) | The `1saves` command-line tool. |

Every memory card filesystem is a crate of its own, because a memory card is a reusable format and
nothing about reading one needs this container. Each depends on nothing at all — not on the rest of
this workspace, not on anything outside it — so a program that just wants to read a card takes one
and stops there.

| Crate | Format |
| ----- | ------ |
| [`ps1-memcard`](crates/ps1-memcard) | PlayStation memory cards, and the containers they ship in |
| [`ps2-memcard`](crates/ps2-memcard) | PlayStation 2 memory cards, ECC and all |
| [`n64-cpak`](crates/n64-cpak) | Nintendo 64 Controller Paks |
| [`neogeo-memcard`](crates/neogeo-memcard) | Neo Geo memory cards, MVS and AES alike |
| [`gc-memcard`](crates/gc-memcard) | GameCube memory cards |
| [`dreamcast-vmu`](crates/dreamcast-vmu) | Dreamcast Visual Memory Units |

What `one-saves-convert` holds for each is the adapter — the mapping between a card's saves and a
bundle's nested parts — and nothing else.

## Features

Everything optional is a feature, and all of them are on by default, so a consumer pays for what it
uses. Reading and writing bundles needs none of them.

| Feature | On | What it brings |
| ------- | -- | -------------- |
| `cards` | `one-saves-convert`, `one-saves-cli` | all six card formats below |
| `ps1` `ps2` `n64` `gc` `vmu` `neogeo` | `one-saves-convert`, `one-saves-cli` | one card crate each, and nothing else |
| `dat` | `one-saves-convert`, `one-saves-cli` | `datary`, `quick-xml`, `serde` and its derive |
| `dat-cmpro` | `one-saves-convert`, `one-saves-cli` | `winnow` |
| `rom` | `one-saves-convert`, `one-saves-cli` | `crc32fast`, `md-5`, `sha1` |
| `zstd` | those two and `one-saves` | `zstd`, and the C library it compiles |

Turning every one off takes 19 crates out of a `1saves` build, the C compile among them:

```console
cargo install one-saves-cli --no-default-features
```

A card format that is off is still **named**: `inspect`, `verify` and `hash` read a bundle of it,
and `convert` and `extract` say which feature would have handled it rather than guessing. What such
a build cannot do is tell that format's dump from a flat save when the extension is ambiguous —
`.bin`, `.srm` — because the reader that knows the signature is what came off.

Each crate's README has the detail.

## What converts

| Format | Extensions | Both directions |
| ------ | ---------- | --------------- |
| Flat cartridge saves | `.srm` `.sav` `.eep` `.fla` `.sa1` | yes |
| PS1 memory cards | `.mcr` `.mcd` `.gme` `.vgs` `.vmp` | yes |
| N64 Controller Paks | `.mpk` `.pak` | yes |
| GameCube memory cards | `.raw` `.gcp` | yes |
| Dreamcast VMU | `.bin` | yes |
| PS2 memory cards | `.ps2`, with or without ECC spare | yes |
| Neo Geo memory cards | `.neo`, bare or in a MiSTer save | yes |

Detection reads the bytes before the extension, because the extension is the less reliable of the
two: the same PS1 card ships as `.mcr`, `.mcd`, `.bin` and `.srm`, and that last one is also what
most libretro cores call a flat cartridge save.

A format this build cannot write declines rather than guessing. A consumer that does not know a
card format cannot rebuild the card and **must not** attempt the write.

A card with nothing saved on it converts too, as a single `card-image` part: a formatted card is
still a real card, and an image is the only part it can have when there is no save to nest.

## Clock state

Emulators append real-time-clock state to the saves of cartridges that have a clock — the Game Boy
MBC3 behind Pokémon Crystal and the Zelda Oracle pair, the Seiko S-3511A behind the GBA Pokémon
titles — and nothing in the file says where the save memory ends. That footer holds a live timestamp, so leaving it
inline gives the save a content hash that moves on its own — which is the one thing a
content-addressable store cannot have.

`1saves` splits it out. The save part becomes the bare save memory and the clock moves into the
header's extension keys, which is where the spec puts it: one cartridge keeps one clock, so it is
said once for the bundle rather than repeated onto every part.

```console
$ 1saves convert crystal.sav --from gambatte --system gb
split a real-time-clock footer off the save, so its bytes hash the same over time

  extension    x.1sav.rtc.mbc3
  clock        mbc3, read at epoch 1700000000
  parts        1
    [0]   save       primary              32768 bytes  09fed9cbfb98b6ab
```

Where the clock lives varies by emulator, and the bundle hides that: mGBA and SameBoy append it to
the save, the MiSTer Gameboy core and its Analogue Pocket ports append it bit-packed, Gambatte
keeps it in a `.rtc` file beside it. All of them arrive as the same two keys, and `extract` puts
each back in the shape its producer writes — down to the reserved size, which is a whole 512-byte
SD block on MiSTer and sixteen bytes on the Pocket.

A sidecar is a special case worth knowing about. It stores the instant the clock counts *from*
rather than what it reads, so turning one into a reading needs to know when it was written — and
the file does not say. `1saves` falls back to the file's modification time, which is right only for
a file an emulator wrote in place; copying, unzipping or checking it out of git all reset it. Pass
`--rtc-captured-at <epoch>` when you know better. Getting it wrong costs the reading, never the
bytes: writing the sidecar back computes `captured_at − elapsed`, which cancels the instant out.

Two keys go on together, as `x.1sav.rtc` prescribes: that key for the portable instant anyone can
read, and `x.1sav.rtc.mbc3` for the chip's own state, from which the footer is regenerated
byte for byte on the way out. Two dumps of that save taken hours apart now agree on the
save's digest and differ only on the clock, which is the truth about them. `--keep-rtc-inline`
restores the naive behaviour.

## Conformance

`one-saves` passes the [conformance corpus][corpus] published alongside the specification: all 24
valid cases and all 49 invalid ones, vendored under
[`crates/one-saves/tests/conformance/`](crates/one-saves/tests/conformance).

A green corpus run is deliberately not full coverage, and its README says so. The four properties
no single file can express are tested directly:

- **Determinism** — every valid case is decoded, re-encoded, and asserted byte-identical.
- **Nesting depth** — a bundle one level past the cap is built and asserted rejected.
- **Unknown values round-trip** — a name or key the decoder does not recognise survives re-encoding.
- **Decoder versus schema** — an integer key from a later minor version is *rejected* by the v0.1
  schema and *accepted and round-tripped* by a shipped decoder. The crate has both modes.

## Determinism

A bundle's content hash is the identifier a content-addressable store keys on, so it has to be a
function of what the bundle says and not of how a producer chose to say it. Two things get it
there, and both are enforced:

- the encoding is deterministic per [RFC 8949 §4.2][det], so a given value has one spelling;
- the data model offers one way to say each thing, so absence is the only empty and the only
  default, and every epoch is whole seconds.

Breaking either is malformed rather than merely unusual, because a consumer that accepted both
spellings would compute two content hashes for one save.

The deterministic CBOR layer is [`dcbor`], which implements the dCBOR application profile. One
divergence is worth knowing: dCBOR adds rules on top of RFC 8949 §4.2 — numeric reduction and a
restricted simple-value set — so a bundle whose *extension value* carries a float or `undefined`
is valid per the specification and rejected by this crate.

## Building

```console
cargo build --workspace
cargo test --workspace
```

There is a [`just`](https://just.systems) file for the rest. `just check` runs what CI runs, in
the order CI runs it, so a green run locally means a green run there:

```console
just            # list the recipes
just check      # fmt, lint, tests, docs, and the registry drift check
just fixtures   # the converters against the real emulator saves under data/
```

The registry data under `crates/one-saves-registry/data/` is synced from the registry pages at
[docs.1retro.com][spec]:

```console
cargo xtask registries --docs ../docs.1retro.com
```

Without `--docs` it rebuilds the Rust tables from the JSON already in the repository, which is what
CI checks. Nothing at build time needs either form.

## Status

Version 0.1, tracking version 0.1 of the specification, which is **not yet stabilized**. Until the
spec reaches 1.0, breaking changes happen in place under the same bundle tag.

## License

Apache-2.0. See [LICENSE](LICENSE).

[spec]: https://docs.1retro.com/specifications/universal-saves-format/
[corpus]: https://github.com/hansl/docs.1retro.com/tree/main/conformance
[det]: https://datatracker.ietf.org/doc/html/rfc8949#name-deterministically-encoded-c
[`dcbor`]: https://docs.rs/dcbor
