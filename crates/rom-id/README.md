# rom-id

Identify a cartridge ROM dump: which system it is for, what its header says, and the hardware on
its cartridge.

```rust,no_run
let mut file = std::fs::File::open("Pokemon - Crystal Version (USA, Europe) (Rev 1).gbc")?;
let name = "Pokemon - Crystal Version (USA, Europe) (Rev 1).gbc";
if let Some(id) = rom_id::identify_file(Some(name), &mut file)? {
    println!("{} {:?}", id.system.slug, id.title);
    if let Some(clock) = id.cartridge.clock {
        println!("its saves carry {}", clock.key());
    }
}
# Ok::<(), std::io::Error>(())
```

## What it reads

A header the console checks settles the system, whatever the file is called: the Nintendo logo on
a Game Boy or GBA, `SEGA` on a Mega Drive, `TMR SEGA` on a Master System or Game Gear, the CRCs a
DS checks, the N64 magic in whichever byte order the dump is in, the containers dumpers wrap NES
(iNES, NES 2.0 and UNIF), Lynx and Atari 7800 ROMs in. A console that checks nothing, such as an Atari 2600 or a PC Engine,
leaves only the file's name, and the answer says so: `Evidence::Name` rather than
`Evidence::Header`. A name whose extension is shared, such as `.bin`, is narrowed by the folder the
file sits in, read through the registry's names for each system. When nothing settles it the
answer is `None`, never a guess.

Beside the system, an answer carries what a save manager needs:

- **The save memory**, its kind and size, from the header where it says: a Game Boy's RAM size and
  battery, a SNES board's SRAM, NES 2.0's battery-backed RAM, a Mega Drive's `RA` record, a
  WonderSwan's footer, an N64 homebrew header. A retail N64 header says nothing about its chip, so
  that comes from the game ID: `n64::cartridge("NSME", None)` answers for every retail game, with
  or without the ROM, and says whether the game also keeps saves in a Controller Pak.
- **The clock chip**, one for each chip key of the Universal Saves Format: the MBC3, HuC-3, TAMA5
  and TPP1 on the Game Boy, the S-RTC and RTC-4513 on the SNES, the Seiko S-3511A on the GBA and
  WonderSwan, the Joybus clock on the N64. The GBA header says nothing about a clock, so those
  come from the game code.
- **The bytes a catalog hashes**: everything after a dumper's or copier's header, and the byte
  order an N64 dump is in, so a No-Intro digest can be computed without guessing.

## I/O is the caller's

The headers sit at a few fixed places near the start of a file and at its end, and which ones
depends only on its length. `Probe::windows` lists them, about a kilobyte in all, so a caller
reading asynchronously, out of an archive or over a network reads them its own way and
`Probe::identify` decides. `identify_file` reads them through the `RomFile` trait, implemented for
a byte slice, a `File` and anything `Read + Seek`; `identify_bytes` takes the whole ROM.

## Saves named by the game

Some systems keep a save under a name the ROM's contents give rather than under the ROM's own
name. A PICO-8 cart saves under the id it passes to `cartdata()`, in a folder every cart shares,
so until it has saved nothing beside it says which save is its. `save_names` reads those names out
of the ROM: the ids a `.p8` or `.p8.png` cart's code passes, a string as it is or a variable
followed to what it is given, and the ids a P8 core's compiled `.p8c` recorded for itself and the
carts it bundles. `names_its_saves` answers from the name alone, so a caller knows which ROMs to
read.

| Feature | What it adds | What it costs |
| ------- | ------------ | ------------- |
| `pico8` (default) | `save_names` for a `.p8` and a `.p8c`, and the `pico8` module | nothing |
| `pico8-png` | a cart in a picture, `.p8.png`, as well; without it one is named and refused | `png`, to read the cart out of the pixels |
