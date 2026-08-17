# dreamcast-vmu

Read and write Dreamcast Visual Memory Unit images.

```rust,no_run
use dreamcast_vmu::Vmu;

let vmu = Vmu::parse(&std::fs::read("vmu.bin")?)?;
for save in vmu.saves() {
    println!("{} ({:?}) {} bytes", save.name, save.kind, save.data.len());
}
```

## The layout

A VMU is 128 KiB of 512-byte blocks.

| Block | What it is |
| ----- | ---------- |
| 255 | the root block, holding the VMU's colour and icon |
| 254 | the allocation table: 256 entries of two bytes |
| 253–241 | the directory: 200 entries of 32 bytes |
| 0–199 | the saves |

The directory grows **downward** from 253, which is the one thing about this layout that catches
people out: entry 16 is the first entry of block 252, not the seventeenth byte-run of a contiguous
region.

A VMU's **data** capacity is the 200 blocks saves can occupy — 102400 bytes — not the 131072 an
image of it runs to.

## Mini-games are not ordinary saves

A mini-game executes in place from flash, so it must start at block 0 and stay contiguous. The
directory entry's type byte says which a save is, so a writer can honour that without understanding
the payload: `VmuBuilder` places every `FileKind::Game` before anything else.

## What round-trips

`Vmu::parse` keeps each save's 32-byte directory entry verbatim, so its timestamp and header offset
survive being read out and written back. `VmuBuilder` regenerates the allocation table and the
directory, and rewrites only the entry's first block, block count and type byte.

No dependencies. Written for [one-saves](https://github.com/one-retro/one-saves), useful on its own.
