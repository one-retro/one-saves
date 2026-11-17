# n64dd-disk

Read and write Nintendo 64DD disk images.

```rust,no_run
use n64dd_disk::Disk;

let image = std::fs::read("game.ndd")?;
let disk = Disk::parse(&image)?;
if let Some(area) = disk.ram_area() {
    std::fs::write("game.ram", area)?;
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

## The disk

A 64DD disk is 4316 blocks. A block's length depends on the zone it sits in, from 19720 bytes on the
outside of the disk to 9520 on the inside, and the disk's type, 0 to 6, decides which zones the
block numbering crosses in which order. The first 24 blocks are the system area: the record giving
the disk's region, type and defect tracks, the disk ID, and blocks the drive refuses to read.

## Two layouts

| Layout | Length | Order |
| ------ | ------ | ----- |
| Retail dump, as No-Intro catalogs it (`.ndd`) | 0x3DEC800 | every block in block order |
| MAME's image | 0x435B0C0 | physical order, head by head and track by track, defect and spare tracks included |

Finding a block in MAME's image takes the defect table in the system record. `Disk` reads either,
and `to_retail` and `to_mame` convert one into the other. The retail form is the one a disk is
hashed in; converting a real MAME image gives the dump No-Intro lists, digest for digest.

## The RAM area

A game writes its saves to the RAM area, which runs from a block the disk type fixes to the disk's
last. mupen64plus, Project64 and MiSTer keep that area alone as a `.ram`. Other emulators keep a
copy of the whole disk, in the layout of the image they were given: `ram_area` reads the area out
of any image, and `with_ram_area` writes one back into a copy.

| Type | First block | Blocks | Bytes |
| ---- | ----------- | ------ | ----- |
| 0 | 1442 | 2874 | 38444480 |
| 1 | 1990 | 2326 | 29501120 |
| 2 | 2538 | 1778 | 21303040 |
| 3 | 3086 | 1230 | 13850240 |
| 4 | 3634 | 682 | 7142720 |
| 5 | 4112 | 204 | 1942080 |
| 6 | — | 0 | no RAM area |

Blocks are numbered as the drive numbers them here; libleo numbers them from the 25th, 24 lower.

The geometry is the one mupen64plus, ares, Project64 and MiSTer share.

No dependencies. Written for [one-saves](https://github.com/one-retro/one-saves), useful on its own.
