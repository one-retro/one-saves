# one-saves-convert

Convert emulator saves and memory cards to and from the [Universal Saves Format][spec].

[`one-saves`](https://docs.rs/one-saves) is the container; this crate is the half that knows about
real hardware.

| Format | Extensions | Filesystem crate | Feature |
| ------ | ---------- | ---------------- | ------- |
| Flat cartridge saves | `.srm` `.sav` `.eep` `.fla` `.sa1` | — | always |
| PS1 memory cards | `.mcr` `.mcd` `.gme` `.vgs` `.vmp` | [`ps1-memcard`](https://docs.rs/ps1-memcard) | `ps1` |
| N64 Controller Paks | `.mpk` `.pak` | [`n64-cpak`](https://docs.rs/n64-cpak) | `n64` |
| GameCube memory cards | `.raw` `.gcp` | [`gc-memcard`](https://docs.rs/gc-memcard) | `gc` |
| Dreamcast VMU | `.bin` | [`dreamcast-vmu`](https://docs.rs/dreamcast-vmu) | `vmu` |
| PS2 memory cards | `.ps2`, with or without ECC spare | [`ps2-memcard`](https://docs.rs/ps2-memcard) | `ps2` |
| Neo Geo memory cards | `.neo`, bare or in a MiSTer save | [`neogeo-memcard`](https://docs.rs/neogeo-memcard) | `neogeo` |

Every format reads and writes. No filesystem is implemented here: each lives in a crate of its own,
because a memory card is a reusable format and nothing about reading one needs this container. What
is here for each is the adapter — the mapping between a card's saves and a bundle's nested parts.

Also here: ROM header parsing for Game Boy, GBC, GBA, N64, SNES, NES and Mega Drive; DAT catalog
resolution in both the Logiqx XML and ClrMamePro shapes; and emulator profiles, so `--from mgba`
knows what to write in a bundle's `source`.

## What a card becomes

A card is a bundle whose `card` map holds the card's own properties and whose parts are one nested
bundle per save, each carrying that save's directory entry verbatim. Extracting one save is then a
byte copy rather than a re-encode.

Every format round-trips: a card read into a bundle and written back out is the same card. What
that does not promise is byte-exact fidelity through a PS2 *rebuild*, where a writer reallocates
the filesystem — keep a `card-image` part if you need that.

## Features

Everything optional is a feature, and all of them are on by default.

| Feature | What it is | What it brings |
| ------- | ---------- | -------------- |
| `cards` | All six card formats at once | the six below |
| `ps1` `n64` `gc` `vmu` `ps2` `neogeo` | One card format each | that format's crate, and nothing else |
| `dat` | The `dat` module: resolving a digest through a DAT catalog | `datary`, and the XML reader, derive macro and proc-macro chain under it |
| `dat-cmpro` | The ClrMamePro syntax as well as Logiqx XML | `winnow` |
| `rom` | The `rom` module: ROM headers and digests | `crc32fast`, `md-5`, `sha1`, `sha2` |
| `zstd` | Forwarded to `one-saves` | `zstd`, and the C library it compiles |

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
one-saves-convert = { version = "0.2", default-features = false, features = ["ps1", "zstd"] }
```

[spec]: https://docs.1retro.com/specifications/universal-saves-format/
