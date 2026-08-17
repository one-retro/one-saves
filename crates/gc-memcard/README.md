# gc-memcard

Read and write GameCube memory card images.

```rust,no_run
use gc_memcard::MemoryCard;

let card = MemoryCard::parse(&std::fs::read("card.raw")?)?;
for save in card.saves() {
    println!("{} ({}{}) {} bytes", save.filename, save.game_code, save.maker_code, save.data.len());
}
```

## The layout

A card is 8 KiB blocks, big-endian throughout, in one of six sizes from 64 to 2048 blocks.

| Block | What it is |
| ----- | ---------- |
| 0 | the header, holding the card's flash serial |
| 1, 2 | the directory and its mirror: 127 entries of 64 bytes |
| 3, 4 | the block-allocation table and its mirror |
| 5+ | the saves |

Each mirror carries an update counter, and the console reads whichever of the pair counted higher.
That is what makes a half-finished write survivable, and this crate reads the same way: the higher
counter wins, and a pair where only one checksums is decided by that instead.

## The two checksums are the opposite way round

The directory's checksum pair sits in its **last** four bytes and covers everything before them. The
table's sits in its **first** four and covers everything after. Conflating the two produces a card
that reads back fine in one implementation and nowhere else.

## What round-trips

`MemoryCard::parse` keeps each save's 64-byte directory entry verbatim. `CardBuilder` regenerates
the directory, the allocation table, both their mirrors and every checksum, and rewrites only the
entry's first block and block count.

A card's **data** capacity is its size minus the five blocks structure takes — never the length of
an image of it.

No dependencies. Written for [one-saves](https://github.com/one-retro/one-saves), useful on its own.
