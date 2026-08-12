# ps2-memcard

Read and write PlayStation 2 memory card images.

```rust,no_run
use ps2_memcard::MemoryCard;

let card = MemoryCard::parse(&std::fs::read("card.ps2")?)?;
for save in card.saves()? {
    println!("{} ({} files)", save.name, save.files.len());
}
```

## Not a FAT volume

A PS2 card rhymes with FAT and is not one. It has Sony's own superblock, a cluster-allocation
table reached through **two** levels of indirection, 512-byte directory entries, and an
error-correcting code in a spare area attached to every page. No FAT library reads it.

| Unit | Size | Notes |
| ---- | ---- | ----- |
| Page | 512 bytes | 528 in a hardware dump, the extra 16 being ECC spare area |
| Cluster | 2 pages | what the allocation table counts |
| Block | 16 pages | the erase unit; the last two are reserved for backups |

A save is a **directory**, not a file: `BASLUS-20312/` holding `icon.sys`, an icon and the game's
own data.

An 8 MB card holds 8388608 bytes of data and dumps to 8650752, because the spare area is invisible
to the filesystem. Both forms are read, and either can be written.

## What round-trips

`MemoryCard::parse` keeps every directory entry's 512 bytes verbatim, so a save's mode bits and
timestamps survive being read out and written back. `CardBuilder` regenerates everything
structural — the allocation table, the directory tree, the superblock and the ECC — because that
is what a writer has to do when saves land at different clusters on the destination card.

So a card built from saves read out of another card holds the same saves, and is not a
byte-for-byte copy of the original. Keep the original image if you need that.

No dependencies. Written for [one-saves](https://github.com/one-retro/one-saves), useful on its own.
