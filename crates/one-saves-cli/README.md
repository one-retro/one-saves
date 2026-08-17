# one-saves-cli

The `1saves` command-line tool: move retro saves and memory cards in and out of the
[Universal Saves Format][spec].

```console
$ cargo install one-saves-cli
```

## Commands

```console
$ 1saves convert card.mcr --from duckstation
card.mcr -> card.1saves (PS1 memory card, 3 save(s), 25525 bytes)

$ 1saves convert pokemon.srm --from mgba --rom pokemon_red.gb --dat No-Intro_GB.dat
identified as "Pokemon - Red Version (USA, Europe)"

$ 1saves inspect card.1saves     # what it is and what it holds, a line per part
$ 1saves verify card.1saves      # structure, every digest, and the encoding
$ 1saves hash card.1saves        # content hash and file hash
$ 1saves extract card.1saves     # write it back out as the card it came from
$ 1saves extract card.1saves --part 1   # pull one save out as its own bundle
$ 1saves profiles                # what --from accepts
```

`--from` names either a producer (`mgba`, `duckstation`, `pcsx2`, or any of the 122 cores in the
registry) or an input format (`ps1`, `raw`). A producer says who wrote the bytes; the format is
still detected from the bytes themselves.

`--rom` reads a ROM's header for a title and product code and hashes it; `--dat` resolves that
digest against a No-Intro, Redump or TOSEC catalog to get a canonical name.

## Features

All on by default. Turning one off drops what it carries, and the dependencies under it, from the
binary.

| Feature | What it carries | What comes off |
| ------- | --------------- | -------------- |
| `cards` | all six card formats | the six crates below |
| `ps1` `n64` `gc` `vmu` `ps2` `neogeo` | one card format each | that format's crate, and nothing else |
| `dat` | `--dat FILE` | `datary` and the XML reader, derive macro and proc-macro chain under it |
| `dat-cmpro` | the ClrMamePro syntax for `--dat` | `winnow` |
| `rom` | `--rom FILE` | `crc32fast`, `md-5`, `sha1` |
| `zstd` | — | `zstd` and the C library it compiles |

`dat` implies `rom`: a catalog is looked up by the ROM's digest, so `--dat` has nothing to resolve
without `--rom`. With `zstd` off, `hash` and `verify` report a content hash as unavailable on a
bundle whose payload is compressed rather than computing one.

A card format that is off is still **named**. `inspect`, `verify` and `hash` read a bundle of it
unchanged, and `convert` and `extract` refuse in these words rather than guessing:

```console
$ 1saves convert card.mcr
1saves: this is a PS1 memory card, and this build has the `ps1` feature off, so it can
        name the format and not read or write it
```

What such a build cannot do is tell that format's dump from a flat save when the extension is
ambiguous — `.bin`, `.srm` — because the reader that knows the signature is what came off.

```console
$ cargo install one-saves-cli --no-default-features --features ps1,zstd
```

[spec]: https://docs.1retro.com/specifications/universal-saves-format/
