# one-saves-registry

The registries the [Universal Saves Format][spec] draws its vocabularies from: 49 systems, 122
emulator cores, save roles, card formats, device kinds, bindings and vendor names.

```rust
use one_saves_registry::{system_by_any_name, core_by_any_name, role, RoleMatch};

// A libretro saves folder, a MiSTer core name and a database label all land on one slug.
assert_eq!(system_by_any_name("megadrive").unwrap().slug, "genesis");
assert_eq!(core_by_any_name("Genesis_MiSTer").unwrap().slug, "megadrive-mister");

// A numbered socket resolves past what is listed: the prefix is registered once.
assert!(matches!(role("memcard-8"), RoleMatch::Numbered(_, 8)));
```

A producer that already knows which core it is skips the lookup, and skips the table with it:

```rust
use one_saves_registry::{ClockLayout, cores, systems};

assert_eq!(cores::MGBA.slug, "mgba");
assert_eq!(cores::MGBA.clock, Some(ClockLayout::Appended));
assert!(cores::MGBA.systems.contains(&systems::GBA.slug));
```

Cores and systems are consts rather than rows reached through a table, so naming one links that
entry alone: about 40 bytes, against roughly 17 KB for the searchable core table. Dead-stripping
is per table, so a consumer that never calls `vendor()` does not carry the vendor list either.

## Why it exists

Five of the format's fields take a name a producer **cannot** mint: a header's `system`, a card's
`format`, a part's `role` and `binding`, and a source's `device_kind`. Those name categories
everyone shares, so minting into one would not extend it but split it. This crate is those
vocabularies, as data.

The **also seen as** column of each registry records what other tools call the same thing —
directory names, core names, database labels. They exist so a producer can map its input onto the
right slug, and they are never emitted.

The tables are non-normative and first-come, so a name absent here is not thereby invalid:
`system()` returning `None` means "not listed", never "malformed".

One field is not drawn from the specification pages: `Core::clock`, which records how a core lays
real-time-clock state down beside a save. The specifications do not describe clock layouts, so
those come from files real cores wrote and live in `data/clocks.json`, which a `--docs` sync
leaves alone.

No dependencies.

[spec]: https://docs.1retro.com/specifications/universal-saves-format/
