# saturn-backup

Read and write Sega Saturn backup RAM: the console's internal memory and the Backup RAM Cart alike.

```rust,no_run
use saturn_backup::BackupRam;

let volume = BackupRam::parse(&std::fs::read("save.bkr")?)?;
for save in volume.saves() {
    println!("{} ({} bytes): {}", save.name, save.data.len(), save.comment);
}
```

## The volume states its own block size

Every other card format has a block size fixed by the format. This one does not: the console's
internal memory allocates in 64-byte blocks and a 512 KiB cart in 512-byte ones, and the same
filesystem runs on both.

The volume says which, and it says so in the signature. `BackUpRam Format` is sixteen bytes written
over and over to fill block 0 exactly, so the number of times it repeats **is** the block size
divided by sixteen — four times for a 64-byte block, thirty-two for a 512-byte one. `Geometry::of`
reads that rather than inferring anything from the file's length, which is what lets a dump of
either medium be read without being told which it is.

| medium | volume | block | blocks | signature repeats |
| --- | --- | --- | --- | --- |
| internal | 32 KiB | 64 | 512 | 4× |
| cart | 512 KiB | 512 | 1024 | 32× |
| cart | 1 MiB | 512 | 2048 | 32× |
| cart | 2 MiB | 512 | 4096 | 32× |
| cart | 4 MiB | 1024 | 4096 | 64× |

The first two are confirmed against real dumps; the rest come from the backup library's own table,
where a cart is `0x40000 << (id & 0x0F)` bytes with 1024-byte blocks on the 4 MiB one and 512 on
the others. `GEOMETRIES` carries all five.

Note the 4 MiB cart: it has the same block *count* as the 2 MiB one and twice the block size, so a
reader inferring geometry from length alone would get it wrong. Reading the repeat count doesn't.

## A block is 60 bytes, not 64

Every block opens with a four-byte tag: `80 00 00 00` on the first block of a save, `00 00 00 00`
on every later block of it. What is left is the content area, which is four bytes short of the
block.

This is the detail that decides whether a reader is right. A save small enough to fit one block
reads correctly either way; every longer one is wrong if the tags are folded into the payload, and
wrong by four bytes per block, which looks like corruption at the end rather than a layout error.

## The directory is inside the save

There is no directory region. A save's own first block is its entry, and the thirty bytes of it
ahead of the block list are a `dirent` like any other format's — nothing in them depends on where
the save sits:

| Offset | Bytes | What |
| ------ | ----- | ---- |
| `0x00` | 4 | the tag |
| `0x04` | 11 | the name, NUL-padded — `PANDRA_3_01` |
| `0x0F` | 1 | the language the game wrote it under |
| `0x10` | 10 | the comment the BIOS lists beside the name — `AZEL#1Lv01` |
| `0x1A` | 4 | when it was written, big-endian minutes since 1980-01-01 |
| `0x1E` | 4 | how many bytes of it are the game's |
| `0x22` | … | the block list, big-endian 16-bit, terminated by `0x0000` |

The block list runs past the end of its own block for a save of any real size, carrying on in the
next listed block's content area — after that block's tag. The save's data starts immediately after
the terminator, wherever in whichever block that lands, and runs on through every listed block.

So the payload `Save::data` hands back is the game's bytes and nothing else. Nothing about the
entry survives in it, which is what makes writing one a rewrite rather than a copy.

## What a writer regenerates

All of it. The block numbers are *inside* the region being written, so moving a save renumbers its
own list — this is the one card format where a writer cannot treat the payload as opaque.
`BackupBuilder` hands out a run of blocks and writes the list to match.

A run is always a valid answer, but it is not the only one: the list is a list precisely so the
blocks need not be adjacent, and `BackupRam::parse` follows the numbers rather than walking
forward. A list that loops or leaves the volume is refused rather than read.

`BackupBuilder::add_at` places a save on exactly the blocks you name, which is how to reproduce a
volume off a console that fragmented — or to build one, since the allocator here never will.

## What is reserved

Blocks 0 and 1. Block 0 is the signature; block 1 holds nothing and is never allocated.

Nothing in a volume says so, and it isn't guesswork: the backup library reserves two blocks and
says so twice. Yabause's HLE BIOS, which reimplements that library, looks for saves from
`2 * blocksize` and reports free space as `(size / blocksize) - 2 - usedblocks`. What block 1 is
*for* isn't recorded anywhere — the library simply never hands it out.

Free blocks read back as zeros, so a formatted volume is the signature and nothing else.

## Not the `.BUP` file format

A `.BUP` file is one save with a 64-byte header in front of it, which is what a save manager
exports. It describes the same save and is a different format; this crate does not read one.

## Two containers

Producers disagree about whether a dump is the volume or the address space around it.

| | length for a 32 KiB volume | shape |
| --- | --- | --- |
| `Container::Packed` | 32 KiB | the data |
| `Container::Wide` | 64 KiB | each byte on an odd address, behind the `0xFF` an even one reads |

The memory is eight bits wide on a sixteen-bit bus, so the even addresses answer to nothing. Beetle
Saturn writes packed; Yabause writes wide. `strip_container` tells them apart and `parse` takes
either, so nothing above this has to know which arrived. `BackupBuilder` writes back the one it was
given, which is what lets a wide dump come back byte for byte.

Neither carries anything the other does not — widening is `0xFF` before each byte and nothing else
— so moving a volume between the two is lossless in both directions.

## Compression

Standalone Mednafen gzips the Backup RAM Cart it writes, and leaves the internal memory plain. A
gzip member is recognised and refused with `Error::Compressed` rather than read as a broken volume
— this crate depends on nothing, and a memory card is not the place to acquire a compression
library. Inflate it first and it is an ordinary volume.

## Extensions in the wild

| Producer | internal | cart | container |
| --- | --- | --- | --- |
| Beetle Saturn | `.srm` | `.bcr` | packed |
| Yabause | `.srm` | | wide |
| Mednafen | `.bkr` | `.bcr`, gzipped | packed |

None of those extensions is a signature, so `detect` reads the bytes. All three agree on the
filesystem: same block sizes, same entries, same allocation.

No dependencies by default; the optional `gzip` feature brings `flate2` on the pure-Rust `zlib-rs`
backend to inflate a compressed cart — the same inflater the `zip` crate uses, so a consumer that
already reads archives gains nothing new. Written for [one-saves](https://github.com/one-retro/one-saves), useful on its own.
