# one-saves-convert

Convert emulator saves, memory cards and save states to and from the [Universal Saves Format][spec].

[`one-saves`](https://docs.rs/one-saves) is the container; this crate is the half that knows about
real hardware.

| Format | Extensions | Filesystem crate | Feature |
| ------ | ---------- | ---------------- | ------- |
| Flat cartridge saves | `.srm` `.sav` `.eep` `.fla` `.sa1` | — | always |
| Sega CD backup RAM | `.brm` | — | always |
| PS1 memory cards | `.mcr` `.mcd` `.gme` `.vgs` `.vmp` | [`ps1-memcard`](https://docs.rs/ps1-memcard) | `ps1` |
| N64 Controller Paks | `.mpk` `.pak` | [`n64-cpak`](https://docs.rs/n64-cpak) | `n64` |
| GameCube memory cards | `.raw` `.gcp` | [`gc-memcard`](https://docs.rs/gc-memcard) | `gc` |
| Dreamcast VMU | `.bin` | [`dreamcast-vmu`](https://docs.rs/dreamcast-vmu) | `vmu` |
| PS2 memory cards | `.ps2`, with or without ECC spare | [`ps2-memcard`](https://docs.rs/ps2-memcard) | `ps2` |
| Neo Geo memory cards and a Neo Geo CD's memory | `.neo`, bare or in a MiSTer save | [`neogeo-memcard`](https://docs.rs/neogeo-memcard) | `neogeo` |
| RetroArch, mGBA and DuckStation save states | `.state*`, `.ss0`…`.ss9`, DuckStation's `.sav` | — | `states` |
| Screenshots | `.png` `.jpg` | — | always |

A backup RAM is wrapped whole: its volume footer is read as a signature, which settles `system`
and nothing else, and its filesystem is not taken apart. That is why it sits with the flat saves
and has no crate of its own.

A Neo Geo CD takes no card and keeps its saves in an 8 KiB one built into the console. A card is
read as that, under `neogeo-cd` and the `internal` role, when `CardOptions::system` says so, or
when it is the 8 KiB card in a MiSTer save, which the core holds only in CD mode. A bare card does
not say what wrote it, so without either it is read as a cartridge console's, under `neogeo`.

Every format reads and writes. No filesystem is implemented here: each lives in a crate of its own,
because a memory card is a reusable format and nothing about reading one needs this container. What
is here for each is the adapter — the mapping between a card's saves and a bundle's nested parts.

A save state is not a save, and `state` keeps them apart: it reads an emulator's state file into a
`state` bundle naming the core the state loads into, and rebuilds the file only for an emulator
that runs that core. The core's data is the state part, uncompressed and out of whatever the
emulator wrapped around it. A screenshot rides beside it as an image, a PNG where the emulator kept
a bare framebuffer; mGBA's in-game save is a save part of its own, and its cheats an extension key
on the state part. RetroArch's file does not name its core, so `StateOptions::source` has to.

Also here: ROM header parsing for Game Boy, GBC, GBA, N64, SNES, NES and Mega Drive; DAT catalog
resolution in both the Logiqx XML and ClrMamePro shapes; and emulator profiles, so `--from mgba`
knows what to write in a bundle's `source`.

## What a card becomes

A card is a bundle whose `card` map holds the card's own properties and whose parts are one nested
bundle per save, each carrying that save's directory entry verbatim. Extracting one save is then a
byte copy rather than a re-encode. What a console keeps on the card for itself — a PS2's browser
history, its network settings, a system update — is no game's save, so it rides beside them as
`system` parts, one per file or directory, and every save on a card is a game's.

Every format round-trips: a card read into a bundle and written back out is one a console loads
exactly as it loaded the original — every directory in its order, every entry, every byte of every
file. It is not the same bytes. A writer allocates its own blocks, where a console's placement
depends on everything ever written to the card, and leaves free blocks erased rather than holding
whatever a deleted save left — keep a `card-image` part if you need those.

A card with nothing on it is carried as a single `card-image` part, and `card::blank` makes one
from nothing: any size the format comes in (`card::sizes`), a GameCube card for a Japanese console's
Shift-JIS, and a Saturn volume or Neo Geo card wrapped as Yabause or a MiSTer keeps it.
`card::blank_like` empties a card the way formatting it on a console would, keeping its size.

## Features

Everything optional is a feature. `icon`, `archive`, `states` and `multithread` are off by default;
the rest are on. The four that are off are the four that would make a consumer compile something it
may have no use for — compression libraries, a PNG encoder and a set of atomic refcounts.

| Feature | What it is | What it brings |
| ------- | ---------- | -------------- |
| `cards` | All seven card formats at once | the seven below |
| `ps1` `n64` `gc` `vmu` `ps2` `neogeo` `saturn` | One card format each | that format's crate, and nothing else |
| `dat` | The `dat` module: resolving a digest through a DAT catalog | `datary`, and the XML reader, derive macro and proc-macro chain under it |
| `dat-cmpro` | The ClrMamePro syntax as well as Logiqx XML | `winnow` |
| `rom` | The `rom` module: ROM headers and digests | `crc32fast`, `md-5`, `sha1`, `sha2` |
| `shift-jis` | Titles a console stored in Shift-JIS | `encoding_rs` |
| `zstd` | Forwarded to `one-saves` | `zstd`, and the C library it compiles |
| `icon` (off) | Decoding a console's icon to PNG for `x.1sav.icon` | `png`, and a deflate encoder |
| `archive` (off) | The `archive` module: reading a card, or the loose saves off one, out of a `.zip` | `zip`, and a deflate decoder |
| `states` (off) | Reading and writing RetroArch's, mGBA's and DuckStation's save states | `flate2` and `png`; with `zstd`, a DuckStation state as DuckStation compresses it by default |
| `multithread` (off) | Forwarded to `one-saves`: `Arc` rather than `Rc` under `dcbor` | nothing — it makes what this crate returns `Send` and `Sync` |

Flat saves, detection, profiles and clock handling are always present: they are what this crate is,
and they depend on nothing but `one-saves`. With everything off, the crate builds against
`one-saves`, `one-saves-registry` and `dcbor` alone.

`Format` keeps every variant whichever card features are on, so a downstream `match` compiles the
same way in every build. What a format's feature takes away is the reader and the writer:
`card::read` and `card::write` then return `Error::Unsupported`, and `detect` can no longer
recognise that format by its **signature**. Its extensions still resolve, so a `.mcr` handed to a
build without `ps1` is named as a PS1 card and refused rather than misread; what such a build
cannot do is tell a card dumped under an ambiguous name — `.bin`, `.srm` — from the flat save those
usually mean.

Without `dat-cmpro` a ClrMamePro catalog is refused, and the error names the feature that would
have read it rather than reporting the file as broken.

```toml
one-saves-convert = { version = "0.6", default-features = false, features = ["ps1", "zstd"] }
```

[spec]: https://docs.1retro.com/specifications/universal-saves-format/
