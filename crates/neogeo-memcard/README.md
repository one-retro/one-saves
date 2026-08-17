# neogeo-memcard

Read and write Neo Geo memory card images.

```rust,no_run
use neogeo_memcard::MemoryCard;

let card = MemoryCard::parse(&std::fs::read("card.neo")?)?;
for save in card.saves() {
    println!("NGH {:04X} save {}: {}", save.ngh, save.sub, save.title());
}
```

## One card, two consoles

The MVS cabinet and the AES home console take the same card, and a save written on one is read by
the other — which is the whole point of it. Nothing on the card records which wrote it, so nothing
here distinguishes them either.

## The layout

A card is 64-byte blocks, and how many of them each region takes comes from the card's own size
word rather than being fixed. One directory entry per block, one table byte per block:

| | 2 KiB card (cartridge) | 8 KiB card (Neo Geo CD) |
| --- | --- | --- |
| header | block 0 | block 0 |
| directory | blocks 1–2, 32 entries | blocks 1–8, 128 entries |
| allocation table 1 | block 3 | blocks 9–10 |
| allocation table 2 | block 4 | blocks 11–12 |
| save data | block 5 onward | block 13 onward |

The header holds the size, both table checksums, the username, the `NEO-GEO` signature and the
region byte. Both layouts are confirmed against real dumps.

A directory entry is `[sub-number][NGH number, big-endian BCD][first block]`, and `$FF` in the
first byte means the slot is free. The NGH number is the game's — Fatal Fury 2 is `0x0047`,
Metal Slug X is `0x0250` — and one game may keep up to 16 saves, told apart by the sub-number:
Metal Slug X writes its stage progress as sub 0 and its options as sub 1.

`Geometry` works the regions out and `MemoryCard::parse` checks the result against the table's own
reserved marks, refusing a card the two disagree about rather than reading through wrong offsets.

## The size word is an address span

The size at `$0A` is **twice** the card's length in bytes — `$1000` for a 2 KiB card, `$4000` for
an 8 KiB one. The card is 8-bit and the bus is 16, so what it records is how much of the 68000's
address space it answers to. Everything else is counted in bytes.

## The allocation table is a chain

One byte per block, and the byte is the *next* block's number. `0` is free, `2` is reserved, `1`
says this is the last block of its save, and anything else is where the save carries on. An entry
names only the first block and carries no length, so the chain is the only thing that says how long
a save is — and its blocks need not be adjacent.

The three sentinels never collide with a real block number, because the first block a save can
occupy is 5 on the smallest card and 13 on the largest.

The table's checksum, which the header carries twice, is the plain 8-bit sum of its bytes. Four real
cards agree with it at four different values.

## What "erased" looks like

Not zeros. A block no save occupies reads back twice its own offset, big-endian, so the run at
`$180` is `03 00 03 04 03 08` — the address the byte answers to showing through, which is the same
thing the filler bytes inside the signature are. `CardBuilder` reproduces it, which is what lets a
card a console wrote come back byte for byte.

## Containers

A bare card is the card and nothing else. The MiSTer NeoGeo core writes something larger, and its
RTL is where this was checked rather than guessed:

| Bytes | What | Where the core says so |
| ----- | ---- | ---------------------- |
| `0x00000`–`0x0FFFF` | cabinet backup RAM, 64 KiB | `sram_wr = ~bk_lba[7]`, `rtl/mem/backup.v` |
| `0x10000`–`0x11FFF` | the memory card, 8 KiB | `memcard_wr = bk_lba[7]`, `rtl/mem/memcard.v` |

The transfer runs to `bk_lba >= 'h8F` — 144 sectors of 512 bytes, which is the 73728 a save comes
to. The card half is written as 16-bit words whose high byte is the card's even address and whose
low byte is its odd one, so swapping the two bytes of every word turns the file back into the
address space the 68000 sees.

`Container` tells the two apart, `parse` accepts either, and the backup RAM comes back from
`backup_ram()` rather than being dropped — it is the cabinet's own saves, which is a different
thing from the card's.

For carts with extra RAM the core puts *that* in the card's region instead, so a save for one holds
no card. `detect` wants the signature and says no, which is the right answer rather than a near
miss.

No dependencies. Written for [one-saves](https://github.com/one-retro/one-saves), useful on its own.
