# ps1-memcard

Read and write PlayStation memory card images.

```rust,no_run
use ps1_memcard::MemoryCard;

let card = MemoryCard::parse(&std::fs::read("card.mcr")?)?;
for save in card.saves() {
    println!("{} in slot {} ({} bytes)", save.name, save.slot, save.data.len());
}
```

## One card, five extensions

The same 131072 bytes ship as `.mcr`, `.mcd`, `.bin` and `.srm` bare, and behind a header as `.gme`
(DexDrive), `.vgs` (VGS/Connectix) and `.vmp` (PSP). `strip_container` takes the wrapper off, which
is why this is one reader rather than five.

## One save, on its own

A save also travels without a card: `.psv` is what a PS3 or a Vita exports, `.mcs` what most PC
tools write. `read_single` reads either into a `Save` a `CardBuilder` can file onto a card.

```rust,no_run
use ps1_memcard::{CardBuilder, read_single};

let save = read_single(&std::fs::read("BASLUS-01476.PSV")?).expect("a save");
let card = CardBuilder::new().add(save).build()?;
```

The name comes out of the container, not the filename: exporters hex-escape anything outside
`[A-Za-z0-9]`, so the save a console calls `BASLUS-01363-00002` arrives as
`BASLUS-013632D3030303032.PSV`. A `.psv` holding a PS2 save is refused rather than read — that one
is a directory of files, not a run of blocks.

## The layout

| Unit | Size | Notes |
| ---- | ---- | ----- |
| Frame | 128 bytes | the header block's unit, and a directory entry's length |
| Block | 8192 bytes | what a save occupies; a save can span several |
| Card | 16 blocks | block 0 is the header, blocks 1–15 the saves |

Block 0 is 64 frames: frame 0 is the `MC` magic, frames 1–15 are the directory — one entry per data
block — and the rest is the broken-sector list and a write-test copy of frame 0. A save's blocks are
chained through a link field rather than laid out contiguously.

## What round-trips

`MemoryCard::parse` keeps each save's 128-byte directory entry verbatim, so whatever the entry says
beyond the fields modelled here survives being read out and written back. `CardBuilder` regenerates
everything structural — the block chains, the directory and every frame checksum.

What is deliberately not recorded is where a save's blocks sat. No console addresses a save by block
and every tool that moves saves between cards throws that field away, so a builder allocates fresh
blocks and packs from the front.

No dependencies. Written for [one-saves](https://github.com/one-retro/one-saves), useful on its own.
