# one-saves

Rust implementation of the [Universal Saves Format][spec] (`.1saves`, `1SAV`): a portable,
self-describing CBOR container for retro console save files and everything attached to them.

A retro save is a bare binary dump with no agreed way to say what game it belongs to, what wrote
it, or what else belongs with it. Every emulator invented its own convention. This is the
container that fixes that, plus the tools to move real saves and memory cards in and out of it,
and the save states and screenshots an emulator keeps beside them.

```console
$ 1saves convert card.mcr --from duckstation
card.mcr -> card.1saves (PS1 memory card, 2 save(s), 58034 bytes)

$ 1saves inspect card.1saves
card.1saves
  58034 bytes, spec version 0.6
  shape        card
  system       psx (PlayStation)
  card         ps1-mc, capacity 131072 bytes
               system area h'4d430000000000000000000000000000…' (8192 bytes)
  source       core duckstation (DuckStation), emulator
  game         unidentified
  parts        2
    [0]   bundle     memcard-1            41038 bytes  f78b86a6953ca4ce  SCUS-94194   slot 1   BASCUS-94194GT
          dirent       h'5100000000a000000100424153435553…' (128 bytes)
            shape        save
            system       psx (PlayStation)
            game         SCUS-94194
            parts        1
              [0]   save       primary              40960 bytes  2360ed4a31caa17b
    [1]   bundle     memcard-1             8270 bytes  d97147227506d5bc  SCUS-94154   slot 6   BASCUS-9415400827918
          dirent       h'5100000000200000ffff424153435553…' (128 bytes)
            shape        save
            system       psx (PlayStation)
            game         SCUS-94154
            parts        1
              [0]   save       primary               8192 bytes  67e275405e9e84ee
```

Each part line is its id, kind, socket, size, the head of its digest, the game's product code, the
slot it sat in and the name the card held. The name goes last because it is the one field with no
useful bound — a GameCube filename runs to 32 characters — so everything before it stays in column.
Two saves of one game share a product code and are told apart by the rest.

Beneath each part line is everything else the part says: here the card's own directory entry,
elsewhere its producer where that differs from the bundle's, its extension keys, a binding. A save
on a card is a bundle of its own, so beneath that comes the bundle it holds, listed the same way.

## The crates

| Crate | What it is |
| ----- | ---------- |
| [`one-saves`](crates/one-saves) | The format: decode, encode, validate, content hash. |
| [`one-saves-registry`](crates/one-saves-registry) | Systems, emulator cores, apps, platforms, save roles, vendor names. |
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
| [`neogeo-memcard`](crates/neogeo-memcard) | Neo Geo memory cards, MVS and AES alike, and the one a Neo Geo CD keeps built in |
| [`gc-memcard`](crates/gc-memcard) | GameCube memory cards |
| [`dreamcast-vmu`](crates/dreamcast-vmu) | Dreamcast Visual Memory Units |
| [`saturn-backup`](crates/saturn-backup) | Sega Saturn backup RAM, internal and Backup RAM Cart |

What `one-saves-convert` holds for each is the adapter — the mapping between a card's saves and a
bundle's nested parts — and nothing else.

Two more read what a game's own media says about it, for a program sorting a library of dumps or
pairing a save with the game it belongs to. Each depends on `one-saves-registry`, for the systems
it names, and on nothing else.

| Crate | What it identifies |
| ----- | ------------------ |
| [`rom-id`](crates/rom-id) | A cartridge ROM: its system, its header, its save memory and clock chip |
| [`disc-id`](crates/disc-id) | A CD or DVD image: its system and the serial printed on the disc |

## Features

Everything optional is a feature, and all of them are on by default except `multithread`, so a
consumer pays for what it uses. Reading and writing bundles needs none of them.

| Feature | On | What it brings |
| ------- | -- | -------------- |
| `cards` | `one-saves-convert`, `one-saves-cli` | all seven card formats below |
| `ps1` `ps2` `n64` `gc` `vmu` `neogeo` `saturn` | `one-saves-convert`, `one-saves-cli` | one card crate each, and nothing else |
| `dat` | `one-saves-convert`, `one-saves-cli` | `datary`, `quick-xml`, `serde` and its derive |
| `dat-cmpro` | `one-saves-convert`, `one-saves-cli` | `winnow` |
| `rom` | `one-saves-convert`, `one-saves-cli` | `crc32fast`, `md-5`, `sha1` |
| `states` | `one-saves-cli`; off in `one-saves-convert` | `flate2` and `png`, for the state files emulators compress and the framebuffers they keep |
| `zstd` | those two and `one-saves` | `zstd`, and the C library it compiles |
| `multithread` | off; `one-saves-convert` and `one-saves` | nothing — `Arc` rather than `Rc` under `dcbor`, so a `Bundle` is `Send` and `Sync` |

`multithread` is the one that is off by default: it buys nothing unless a bundle has to cross a
thread or sit in a `Send` future, and it makes `dcbor`'s reference counts atomic. Turning every
*other* one off takes 19 crates out of a `1saves` build, the C compile among them:

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
| Sega CD backup RAM | `.brm` | yes, whole |
| PS1 memory cards | `.mcr` `.mcd` `.gme` `.vgs` `.vmp` | yes |
| N64 Controller Paks | `.mpk` `.pak` | yes |
| GameCube memory cards | `.raw` `.gcp` | yes |
| Dreamcast VMU | `.bin` | yes |
| PS2 memory cards | `.ps2`, with or without ECC spare | yes |
| Neo Geo memory cards and a Neo Geo CD's memory | `.neo`, bare or in a MiSTer save | yes |
| Saturn backup RAM | `.bkr` `.bcr`, internal and Backup RAM Cart | yes |
| RetroArch save states | `.state`, `.state1`…, `.state.auto`, and the `.png` beside | yes |
| mGBA save states | `.ss0`…`.ss9` | yes |
| DuckStation save states | `.sav` and `.bak` under `savestates` | yes |
| Screenshots | `.png` `.jpg` | yes, as written |

Detection reads the bytes before the extension, because the extension is the less reliable of the
two: the same PS1 card ships as `.mcr`, `.mcd`, `.bin` and `.srm`, and that last one is also what
most libretro cores call a flat cartridge save.

Saturn is the one card whose block size is not a property of its format: the console's internal
memory allocates in 64-byte blocks and a Backup RAM Cart in 512-byte ones. The volume states which
— `BackUpRam Format` repeats to fill block 0 exactly, so the repeat count is the block size over
sixteen — so a dump of either is read without being told which it is. A Saturn save also carries its own entry, and
its own list of the blocks it occupies, at the head of its first block rather than in a directory
region — so a writer renumbers that list while the entry ahead of it rides as an ordinary
`dirent`.

A backup RAM is the one flat save that names itself. A volume ends in a 64-byte footer whose
second half never varies — the volume name and the free-block and file counts are in the first
half, and those move as saves come and go — so a dump converts with its `system` already filled in
whatever the file is called — and a Sega CD dump under `.bin` is a file nothing else here would have recognised. What
that does *not* do is read the volume's filesystem: the bundle holds the whole 8 KiB, or the whole
Backup RAM Cart, as one part rather than one part per save. A console carries both sockets at once,
so say which one a dump came out of with `--role internal` or `--role ram-cart`.

A format this build cannot write declines rather than guessing. A consumer that does not know a
card format cannot rebuild the card and **must not** attempt the write.

A card with nothing saved on it converts too, as a single `card-image` part: a formatted card is
still a real card, and an image is the only part it can have when there is no save to nest.

Not everything on a card is a save. A PS2 keeps its browser history, its network settings and any
system update in folders of its own, and those ride beside the saves as `system` parts, so every
save on a card names the game it belongs to. A MiSTer NeoGeo save whose cabinet RAM holds anything
converts as a `device` of the RAM and whatever sits beside it, the card or a cartridge's own RAM,
and extracts back to the MiSTer save.

## Save states and screenshots

A save state is not a save. The game wrote a save, and anything that runs the game reads it; an
emulator took a state, the whole machine at one moment, and only the core that took it loads it.
The format carries a state as a shape of its own, so a consumer restoring a save never meets one,
and its `source` names the core it loads into.

```console
$ 1saves convert "states/Genesis Plus GX Wide/Sonic CD (USA).state" --system sega-cd
states/Genesis Plus GX Wide/Sonic CD (USA).state: took the core genesis-plus-gx-wide from the folder RetroArch filed it in
states/Genesis Plus GX Wide/Sonic CD (USA).state -> states/Genesis Plus GX Wide/Sonic CD (USA).state.1saves (RetroArch state, slot 0, loads into genesis-plus-gx-wide, 1 part(s), 1036411 bytes)
```

| Emulator | Its files | In the bundle |
| -------- | --------- | ------------- |
| RetroArch | `X.state`, `X.state1`…, `X.state.auto`, and a `.png` thumbnail beside | the core's data, unwrapped and decompressed; the thumbnail as the screenshot; the achievement and replay blocks as `aux` parts |
| mGBA | `X.ss1`…`X.ss9`, and `X.ss0` to resume from | the core's data with the player's cheats on it, the screenshot, the in-game save as a save part, and when and by which mGBA it was taken |
| DuckStation | `SERIAL_N.sav`, `SERIAL_resume.sav`, and `SERIAL_N.bak` for what a slot held before | the core's data decompressed, the version DuckStation stamped on it, the game's title and serial, and the screenshot as a PNG |

The slot is in each file's name, and becomes the state part's `slot`; a state the emulator took on
its own to resume from has none, and an `origin` of `auto`. RetroArch's file does not say which
core took it, so `--core` does, or the folder RetroArch filed the state in when it sorts states by
core. Pass `--rom` as for a save: a state loads only against the ROM it was taken with, and the
ROM's hashes are what finds it again.

`extract` rebuilds the emulator's file, named for its slot, and RetroArch's thumbnail beside it. It
refuses to write a state for an emulator that does not run the core the state names. A rebuilt file
loads, and is not promised byte for byte; a RetroArch state comes back identical once decompressed.

A PNG or a JPEG on its own converts to a `screenshot` bundle, kept exactly as it was written. Any
other image would have to be encoded as PNG first, which is a choice of the producer's rather than
something a converter should do quietly, so it is refused.

## Clock state

Emulators append real-time-clock state to the saves of cartridges that have a clock — the Game Boy
MBC3 behind Pokémon Gold, Silver and Crystal, the Seiko S-3511A behind the GBA Pokémon titles —
and nothing in the file says where the save memory ends. That footer holds a live timestamp, so leaving it
inline gives the save a content hash that moves on its own — which is the one thing a
content-addressable store cannot have.

`1saves` splits it out. The save part becomes the bare save memory and the clock moves into the
header's extension keys, which is where the spec puts it: one cartridge keeps one clock, so it is
said once for the bundle rather than repeated onto every part.

```console
$ 1saves convert crystal.sav --from sameboy --system gb
split a real-time-clock footer off the save, so its bytes hash the same over time

  extension    x.1sav.rtc = 1970-01-01 00:02:13 UTC
  extension    x.1sav.rtc.mbc3 = day 0 00:02:13, taken 2026-08-12 18:24:34
  parts        1
    [0]   save       primary              32768 bytes  fb495f5a287b3ac5
```

Where the clock lives varies by emulator, and the bundle hides that: mGBA and SameBoy append it to
the save, the MiSTer Gameboy and GBA cores and the Gameboy core's Analogue Pocket ports append it
bit-packed, Gambatte and mincer_ray's Pocket GBA core keep it in a `.rtc` file beside it. All of them arrive as the same two
keys, and `extract` puts each back in the shape its producer writes — down to the reserved size,
which is a whole 512-byte SD block on MiSTer and sixteen bytes on the Pocket. `convert` picks up a
`.rtc` named after the save on its own; `extract --rtc-file` writes one.

Gambatte's sidecar is a special case worth knowing about. It stores the instant the clock counts
*from* rather than what it reads. The cartridge clock follows the host's, so `1saves` takes the
counters as they stand when it reads the file, and carries that moment as the anchor. Pass
`--rtc-captured-at <epoch>` to read them at another moment. The choice moves the reading, never the
bytes: writing the sidecar back computes `captured_at − elapsed`, which cancels the instant out.
mincer_ray's records the instant itself, so it needs none of this.

Two keys go on together, as `x.1sav.rtc` prescribes: that key for the portable reading anyone can
read, and `x.1sav.rtc.mbc3` or `x.1sav.rtc.s3511a` for the chip's own state, from which the footer
is regenerated byte for byte on the way out. Two dumps of that save taken hours apart now agree on
the save's digest and differ only on the clock, which is the truth about them. `--keep-rtc-inline`
restores the naive behaviour.

The reading is the chip's own clock on the Unix scale: an MBC3's counters as that long after 1970,
a Seiko chip's calendar as though it were UTC. It is never the host time stored beside them, which
is what the emulator advances the clock from and is on its own scale — MiSTer's is local standard
time counted as though it were UTC. That host time goes in the chip's key, where rebuilding the
footer needs it.

A source that keeps less than the chip holds says so with null rather than a made-up value.
Gambatte's sidecar and the MiSTer core's packed footer keep no latched copy of the MBC3's
registers, and none of the GBA producers keeps the Seiko chip's alarm. A rebuild fills each null
the way the chip's page says to: the live registers for the latched copy, the power-on value for
the alarm.

The MiSTer Gameboy cores and their Pocket ports keep a HuC-3 in the footer they keep an MBC3 in,
packed differently, and nothing in the bytes says which chip wrote it. The cartridge does: pass the
game's ROM with `--rom` and its header names the chip, so `convert` reads a HuC-3's counters and
seconds into `x.1sav.rtc.huc3` and `extract` packs them back. Without the ROM, such a footer is read
as an MBC3 where its bits allow, and left in the save where they do not.

The specification keys every other cartridge clock too: TPP1 and TAMA5 on the Game Boy, the Epson
and Sharp chips on the Super Famicom, and the N64's Joybus clock. `one-saves` has a type for each,
so `inspect` shows them and a producer can write them, but `convert` reads none of their emulators'
files yet, nor any other emulator's HuC-3. A footer for one of them stays in the save, as any tail
this crate cannot identify does.

## Conformance

`one-saves` passes the [conformance corpus][corpus] published alongside the specification: all 37
valid cases and all 92 invalid ones, vendored under
[`crates/one-saves/tests/conformance/`](crates/one-saves/tests/conformance).

A green corpus run is deliberately not full coverage, and its README says so. The four properties
no single file can express are tested directly:

- **Determinism** — every valid case is decoded, re-encoded, and asserted byte-identical.
- **Nesting depth** — a bundle one level past the cap is built and asserted rejected.
- **Unknown values round-trip** — a name or key the decoder does not recognise survives re-encoding.
- **Decoder versus schema** — an integer key from a later minor version is *rejected* by the schema
  of the version this crate implements and *accepted and round-tripped* by a shipped decoder. The
  crate has both modes.

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
just states ~/ROMs:~/states   # the state readers against real states, read where they lie
```

States stay out of the repository, since a state holds the game's own code beside the player's
progress, so `just states` reads them from the folders it is given, separated as `PATH` is.

The registry data under `crates/one-saves-registry/data/` is synced from the registry pages at
[docs.1retro.com][spec], whose source the recipe fetches into `target/spec`:

```console
just sync-registries              # fetch the specifications, then sync from them
just sync-registries ../my-docs   # or sync from a checkout of your own
```

`just registries` rebuilds the Rust tables from the JSON already in the repository instead, which
is what CI checks. Nothing at build time needs either form.

## Status

Tracking version 0.6 of the specification, which is **not yet stabilized**. Until the spec reaches
1.0, breaking changes happen in place under the same bundle tag.

## License

Apache-2.0. See [LICENSE](LICENSE).

[spec]: https://docs.1retro.com/specifications/universal-saves-format/
[corpus]: https://github.com/one-retro/docs/tree/main/conformance
[det]: https://datatracker.ietf.org/doc/html/rfc8949#name-deterministically-encoded-c
[`dcbor`]: https://docs.rs/dcbor
