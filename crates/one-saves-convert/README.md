# one-saves-convert

Convert emulator saves and memory cards to and from the [Universal Saves Format][spec].

[`one-saves`](https://docs.rs/one-saves) is the container; this crate is the half that knows about
real hardware.

| Format | Extensions | Both directions |
| ------ | ---------- | --------------- |
| Flat cartridge saves | `.srm` `.sav` `.eep` `.fla` `.sa1` | yes |
| PS1 memory cards | `.mcr` `.mcd` `.gme` `.vgs` `.vmp` | yes |
| N64 Controller Paks | `.mpk` `.pak` | yes |
| GameCube memory cards | `.raw` `.gcp` | yes |
| Dreamcast VMU | `.bin` | yes |
| PS2 memory cards | `.ps2`, with or without ECC spare | yes |

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

[spec]: https://docs.1retro.com/specifications/universal-saves-format/
