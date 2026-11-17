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

## The `disc-data` fingerprint

What 1Retro keys a disc's saves on: the program, whatever holds it. `disc_data` hashes the user
data of every data track, 2048 bytes a sector, each track with SHA-1, and the fingerprint is
SHA-256 over every track's sector count and SHA-1. Audio is left out, since saves follow the
program and a drive's read offset shifts music between copies of one disc; so are empty sectors at
either edge of a track, which dumps of one disc disagree about. A cue sheet with a file per track
or one image of them all, an `.iso`, and a CHD raw or cooked give the same value, and a translation
that patches any data track gives another.

```rust,ignore
let text = std::fs::read_to_string("games/Game.cue")?;
// Each file the sheet names that holds data is opened through this: a file, or an archive member.
let found = disc_id::disc_data::sheet(&text, |name| {
    // A sheet's names are untrusted: `resolve` keeps them inside the sheet's folder.
    let path = disc_id::cue::resolve(Path::new("games"), name).ok_or(ErrorKind::NotFound)?;
    File::open(path)
})?;
if let Some(found) = found {
    println!("{}", found.hex());
}
```

`disc_data::image` reads an image that came without a sheet, a DVD's included, and
`chd::disc_data` a CHD.

## The Redump hash

Redump publishes a SHA-1 and an MD5 for each track of a disc, over its raw 2352-byte sectors from
the start of its pregap. `redump`
streams the first data track through both, and gives the same value for a cue sheet with a file
per track, one image of every track cut by its sheet, a CloneCD `.img` and a CHD. A DVD is hashed
whole, as Redump hashes one. A cooked track has lost the bytes Redump hashed and has no
fingerprint, and neither does audio.

```rust,ignore
let text = std::fs::read_to_string("games/Game.cue")?;
let track = disc_id::cue::first_data_track(&text).unwrap();
// A sheet's names are untrusted: `resolve` keeps them inside the sheet's folder.
let path = disc_id::cue::resolve(Path::new("games"), &track.file).unwrap();
// A file, or an archive member: only `Read` is asked of it.
if let Some(found) = disc_id::redump::track(&track, File::open(path)?)? {
    println!("{} {}", found.sha1_hex(), found.size);
}
```

## Features

| Feature | What it adds |
| ------- | ------------ |
| `chd` | Reading CHD images, through the `chd` crate and its four decompressors. |
| `disc-data` | The `disc-data` fingerprint of a disc, through the `sha1` and `sha2` crates. With `chd`, of a CHD too. |
| `redump` | The Redump hash of a disc's first data track, through the `sha1` and `md-5` crates. With `chd`, of a CHD too. |
