# n64-cpak

Read and write Nintendo 64 Controller Pak images.

```rust,no_run
use n64_cpak::ControllerPak;

let pak = ControllerPak::parse(&std::fs::read("pak.mpk")?)?;
for note in pak.notes() {
    println!("{} ({}) {} bytes", note.path(), note.game_code, note.data.len());
}
```

## The layout

A pak is 32 KiB of 256-byte pages.

| Page | What it is |
| ---- | ---------- |
| 0 | the ID area, holding the pak's 32-byte label |
| 1, 2 | the index table and its backup |
| 3, 4 | the note table: sixteen 32-byte entries |
| 5–127 | the notes' data |

A note's pages are chained through the index table rather than laid out contiguously, and the
backup table is read when the primary does not parse as chains.

## Note names are not ASCII

A note's name is written in the N64 font's own code table, in which the digits start at 16 and the
letters at 26. Reading one as bytes yields nonsense; the `font` module is the table.

## Two notes can be the same note

A pak permits two notes with the same game code and name, so the note table index is the only thing
that tells them apart. That is why `Note` carries a `slot`.

## What round-trips

`ControllerPak::parse` keeps each note's 32-byte table entry verbatim. `PakBuilder` regenerates the
index table and its backup, and rewrites only the entry's start page — page placement is the
allocator's business and everything else in the entry is the game's.

No dependencies. Written for [one-saves](https://github.com/one-retro/one-saves), useful on its own.
