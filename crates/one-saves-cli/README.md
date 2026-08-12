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

$ 1saves inspect card.1saves     # what it is and what it holds
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

[spec]: https://docs.1retro.com/specifications/universal-saves-format/
