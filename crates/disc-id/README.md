# disc-id

Identify a CD or DVD disc image: which system it is for, and the serial printed on the disc.

```rust,no_run
let file = std::fs::File::open("game.iso")?;
if let Some(id) = disc_id::identify(file)? {
    println!("{} {}", id.system.slug, id.serial.unwrap_or_default());
}
# Ok::<(), std::io::Error>(())
```

A `.bin` and an `.iso` are shared across half a dozen systems, so an extension cannot name a disc's
system, and hashing a multi-track dump answers nothing a catalog keys on. Every disc system
announces itself in its opening sectors instead, and this crate reads those, in bounded kilobytes:
the Sega CD, Saturn and Dreamcast headers, the GameCube and Wii disc header, the PlayStation and
PS2 `SYSTEM.CNF`, the Neo Geo CD and PC Engine CD boot sectors. The system is the registry's, so it
names what a save's bundle carries; the serial is what a memory card entry is keyed on.

## I/O is the caller's

`identify_prefix` takes whatever bytes a caller has from the start of the image and answers with a
`ControlFlow`: the answer, or how many bytes it needs, so it composes with blocking reads, `tokio`,
an inflating zip entry or anything else. `identify` and `identify_seekable` are conveniences over
it for a caller holding a `Read`, and `cue` finds the data track a cue sheet points at.

## Features

| Feature | What it adds |
| ------- | ------------ |
| `chd` | Reading CHD images, through the `chd` crate and its four decompressors. |
