//! Turning the registry JSON into the static tables `one-saves-registry` ships.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::registry::{App, Clock, Clocks, Layout, Layouts, Registries};

/// A Rust string literal.
fn lit(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A Rust slice of string literals.
fn lits(items: &[String]) -> String {
    format!("&[{}]", items.iter().map(|i| lit(i)).collect::<Vec<_>>().join(", "))
}

/// Writes one static table: a doc comment, the array header, a row per item, and the close.
fn table<T>(w: &mut String, doc: &str, name: &str, ty: &str, items: &[T], row: impl Fn(&T) -> String) {
    writeln!(w, "/// {doc}").unwrap();
    writeln!(w, "pub(crate) static {name}: [{ty}; {}] = [", items.len()).unwrap();
    for item in items {
        writeln!(w, "    {},", row(item)).unwrap();
    }
    writeln!(w, "];\n").unwrap();
}

/// The const name a slug takes in the `cores` and `systems` modules: upper snake case, and never
/// leading with a digit, since `3do-mister` is a perfectly good slug and not a Rust identifier.
fn const_name(slug: &str) -> String {
    let body: String =
        slug.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    if body.starts_with(|c: char| c.is_ascii_digit()) { format!("_{body}") } else { body }
}

/// The const names for a set of slugs, refusing any pair that collide.
///
/// Two slugs that differ only where `const_name` does not would emit the same const twice. rustc
/// would catch it, but in generated code and with nothing pointing back at the slugs that caused it.
fn const_names<'a>(slugs: impl Iterator<Item = &'a str>) -> Result<Vec<String>, String> {
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    let mut out = Vec::new();
    for slug in slugs {
        let name = const_name(slug);
        if let Some(first) = seen.insert(name.clone(), slug) {
            return Err(format!("{first:?} and {slug:?} both want the const {name}"));
        }
        out.push(name);
    }
    Ok(out)
}

/// The `ClockLayout` and `Huc3Layout` expressions for one core, each `None` where the file
/// records nothing.
fn clock(slug: &str, clocks: &Clocks) -> Result<(String, String), String> {
    let Some(entry) = clocks.cores.get(slug) else { return Ok(("None".to_owned(), "None".to_owned())) };
    Ok((layout(slug, entry)?, huc3(slug, entry)?))
}

/// The `Option<ClockLayout>` expression for one entry: a core's, or an app's build of it, which
/// `who` names.
fn layout(who: &str, entry: &Clock) -> Result<String, String> {
    let Some(kind) = entry.layout.as_deref() else {
        if entry.reserved.is_some() || entry.timestamp_width.is_some() || entry.record.is_some() {
            return Err(format!("clocks.json: {who:?} describes a clock but names no layout"));
        }
        return Ok("None".to_owned());
    };
    if kind != "sidecar" && (entry.record.is_some() || entry.byte_order.is_some()) {
        return Err(format!("clocks.json: {who:?} names a record, which only a sidecar holds"));
    }
    // A footer stores the host clock at a width the core chose. A sidecar's record may have a
    // width of its own, and the record says whether it does.
    let layout = match (kind, entry.reserved, entry.timestamp_width) {
        ("appended", None, Some(width)) => {
            Ok(format!("crate::ClockLayout::Appended {{ timestamp_width: {width} }}"))
        }
        ("sidecar", None, width) => {
            Ok(format!("crate::ClockLayout::Sidecar({})", record(who, entry, width)?))
        }
        ("packed", Some(n), Some(width)) => {
            Ok(format!("crate::ClockLayout::Packed {{ reserved: {n}, timestamp_width: {width} }}"))
        }
        ("packed", None, _) => Err(format!("clocks.json: {who:?} is packed but sets no reserved length")),
        ("appended" | "packed", _, None) => {
            Err(format!("clocks.json: {who:?} is a footer but sets no timestamp width"))
        }
        ("sidecar", Some(_), _) => Err(format!("clocks.json: {who:?} is a sidecar, which reserves nothing")),
        (other, _, _) => Err(format!("clocks.json: {who:?} has unknown layout {other:?}")),
    }?;
    Ok(format!("Some({layout})"))
}

/// The `Option<Huc3Layout>` expression for one entry's `huc3`.
///
/// Every HuC-3 layout is its own, so each spelling admits exactly the fields it has and no other:
/// a packed footer its reserved length (the host clock in it is MiSTer's four bytes), an origin
/// its width and byte order, the mGBA app's memory and nothing kept nothing at all.
fn huc3(who: &str, entry: &Clock) -> Result<String, String> {
    let Some(huc3) = entry.huc3.as_deref() else { return Ok("None".to_owned()) };
    let who = format!("{who}'s huc3");
    if huc3.huc3.is_some() || !huc3.apps.is_empty() {
        return Err(format!("clocks.json: {who:?} nests what only a core's entry has"));
    }
    let fields = (huc3.reserved, huc3.timestamp_width, huc3.record.as_deref(), huc3.byte_order.as_deref());
    let layout = match (huc3.layout.as_deref(), fields) {
        (Some("packed"), (Some(reserved), None, None, None)) => {
            format!("crate::Huc3Layout::Packed {{ reserved: {reserved} }}")
        }
        (
            Some("sidecar"),
            (None, Some(width @ (4 | 8)), Some("origin"), Some(order @ ("big" | "little"))),
        ) => {
            format!(
                "crate::Huc3Layout::Origin {{ timestamp_width: {width}, big_endian: {} }}",
                order == "big"
            )
        }
        (Some("appended"), (None, None, Some("memory"), None)) => "crate::Huc3Layout::Memory".to_owned(),
        (Some("appended"), (None, None, Some("sameboy"), None)) => "crate::Huc3Layout::SameBoy".to_owned(),
        (Some("appended"), (None, None, Some("vba-m"), None)) => "crate::Huc3Layout::Vbam".to_owned(),
        (Some("none"), (None, None, None, None)) => "crate::Huc3Layout::NotKept".to_owned(),
        (layout, _) => {
            return Err(format!(
                "clocks.json: {who:?} is not one of the HuC-3 layouts: packed with a reserved length, a sidecar \
                 origin with a width of 4 or 8 and a byte order, an appended memory, sameboy or vba-m record, or \
                 none (got {layout:?})"
            ));
        }
    };
    Ok(format!("Some({layout})"))
}

/// Refuses a layout for a core nobody listed, for a system the core does not run, or with no word
/// of where it was read.
fn check_layouts(data: &Registries, layouts: &Layouts) -> Result<(), String> {
    for (slug, entry) in &layouts.cores {
        let Some(core) = data.cores.iter().find(|c| &c.slug == slug) else {
            return Err(format!("layouts.json: {slug:?} is not a listed core"));
        };
        if entry.evidence.trim().is_empty() {
            return Err(format!("layouts.json: {slug:?} says nothing of where its layouts were read"));
        }
        for system in entry.systems.keys() {
            if !core.systems.contains(system) {
                return Err(format!("layouts.json: {slug:?} does not run {system:?}"));
            }
        }
    }
    Ok(())
}

/// A number of bytes as a Rust literal, a long one's digits in threes as clippy asks.
fn separated(n: u32) -> String {
    let digits = n.to_string();
    if digits.len() <= 4 {
        return digits;
    }
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('_');
        }
        out.push(digit);
    }
    out
}

/// The `&[SystemLayout]` expression for one core.
fn save_layouts(slug: &str, layouts: &Layouts) -> Result<String, String> {
    let Some(entry) = layouts.cores.get(slug) else { return Ok("&[]".to_owned()) };
    let mut items = Vec::new();
    for (system, layout) in &entry.systems {
        let layout = save_layout(&format!("{slug}'s {system}"), layout)?;
        items.push(format!("SystemLayout {{ system: {}, layout: {layout} }}", lit(system)));
    }
    Ok(format!("&[{}]", items.join(", ")))
}

/// The `SaveLayout` expression for one entry, refusing a field its layout does not have.
fn save_layout(who: &str, entry: &Layout) -> Result<String, String> {
    use serde_json::Value;
    let length = entry.length.as_ref();
    let fields =
        (entry.fill, length, entry.eeprom_length, entry.reversed_eeprom, entry.eeprom_min, entry.swapped);
    let number = |value: &Value| {
        value.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|&n| n > 0).ok_or_else(|| {
            format!("layouts.json: {who:?} gives a length that is not a number of bytes: {value}")
        })
    };
    let option =
        |value: Option<u32>| value.map_or_else(|| "None".to_owned(), |n| format!("Some({})", separated(n)));
    match (entry.kind.as_str(), fields) {
        ("expanded", (Some(fill), Some(length), eeprom_length, None, None, None)) => {
            let length = match length {
                Value::String(text) if text == "trimmed" => "ExpandedLength::Trimmed".to_owned(),
                Value::String(text) if text == "doubled" => "ExpandedLength::Doubled".to_owned(),
                other => format!("ExpandedLength::Fixed({})", separated(number(other)?)),
            };
            Ok(format!(
                "SaveLayout::Expanded {{ fill: {fill:#04X}, length: {length}, eeprom_length: {} }}",
                option(eeprom_length)
            ))
        }
        ("packed", (None, length, None, None, None, None)) => {
            Ok(format!("SaveLayout::Packed {{ length: {} }}", option(length.map(number).transpose()?)))
        }
        ("padded", (None, Some(length), None, reversed, None, None)) => Ok(format!(
            "SaveLayout::Padded {{ length: {}, reversed_eeprom: {} }}",
            separated(number(length)?),
            reversed.unwrap_or(false)
        )),
        ("vba-next", (None, None, None, None, None, None)) => Ok("SaveLayout::VbaNext".to_owned()),
        ("chip", (None, None, None, None, Some(min), None)) => {
            Ok(format!("SaveLayout::Chip {{ eeprom_min: {} }}", separated(min)))
        }
        // A length is a padding, and comes with what pads it; an entry that neither swaps nor pads
        // keeps the chip as it is, which is what no entry at all says.
        ("mbc7", (fill, length, None, None, None, swapped)) if fill.is_some() == length.is_some() => {
            let swapped = swapped.unwrap_or(false);
            let padding = match (length, fill) {
                (Some(length), Some(fill)) => format!("Some(({}, {fill:#04X}))", separated(number(length)?)),
                _ => "None".to_owned(),
            };
            if !swapped && padding == "None" {
                return Err(format!("layouts.json: {who:?} is an mbc7 layout that changes nothing"));
            }
            Ok(format!("SaveLayout::Mbc7 {{ swapped: {swapped}, padding: {padding} }}"))
        }
        (layout, _) => Err(format!(
            "layouts.json: {who:?} is not one of the layouts: expanded with a fill and a length (and an \
             EEPROM length), packed (with a length), padded with a length (and reversed_eeprom), vba-next, \
             chip with an eeprom_min, or mbc7 (swapped, and a length with a fill) (got {layout:?} with other \
             fields)"
        )),
    }
}

/// The `SidecarRecord` expression for a sidecar entry.
fn record(who: &str, entry: &Clock, width: Option<u8>) -> Result<String, String> {
    let width_of = |record: &str| match width {
        Some(width @ (4 | 8)) => Ok(width),
        Some(other) => Err(format!("clocks.json: {who:?} gives its {record} a {other}-byte timestamp")),
        None => Err(format!("clocks.json: {who:?} keeps a {record} but sets no timestamp width")),
    };
    let fixed = |record: &str, expression: &str| match (width, &entry.byte_order) {
        (None, None) => Ok(expression.to_owned()),
        _ => Err(format!("clocks.json: {who:?} keeps a {record}, whose layout is fixed")),
    };
    match entry.record.as_deref() {
        Some("origin") => {
            let width = width_of("origin")?;
            let big_endian = match entry.byte_order.as_deref() {
                Some("big") => true,
                Some("little") => false,
                _ => return Err(format!("clocks.json: {who:?} keeps an origin but says no byte order")),
            };
            Ok(format!(
                "crate::SidecarRecord::Origin {{ timestamp_width: {width}, big_endian: {big_endian} }}"
            ))
        }
        Some("registers") if entry.byte_order.is_none() => {
            Ok(format!("crate::SidecarRecord::Registers {{ timestamp_width: {} }}", width_of("registers")?))
        }
        Some("registers") => Err(format!("clocks.json: {who:?}'s registers are little-endian by definition")),
        Some("sameboy") => fixed("sameboy", "crate::SidecarRecord::SameBoy"),
        Some("packed-seiko") => fixed("packed-seiko", "crate::SidecarRecord::PackedSeiko"),
        Some(other) => Err(format!("clocks.json: {who:?} keeps an unknown record {other:?}")),
        None => Err(format!("clocks.json: {who:?} is a sidecar but says nothing of what it holds")),
    }
}

/// The table of apps' own builds, a row per build that lays its clock out differently, sorted by
/// core and then app so `Core::clock_in` can binary-search it.
///
/// An entry names an app that ships a build of the core. One for an app that loads the core as
/// its kind would contradict the core's own entry, and one for an app that does not run it at all
/// is a typo.
fn emit_app_clocks(w: &mut String, data: &Registries, clocks: &Clocks) -> Result<(), String> {
    let mut rows = Vec::new();
    for (core, entry) in &clocks.cores {
        for (app_slug, build) in &entry.apps {
            let who = format!("{core} in {app_slug}");
            if !build.apps.is_empty() {
                return Err(format!("clocks.json: {who:?} nests apps, which an app's build cannot have"));
            }
            let app = data
                .apps
                .iter()
                .find(|a| &a.slug == app_slug)
                .ok_or_else(|| format!("clocks.json: {app_slug:?} is not a listed app"))?;
            if !app.cores.iter().any(|c| c == core) {
                return Err(format!("clocks.json: {app_slug:?} ships no build of {core:?} of its own"));
            }
            rows.push(format!(
                "crate::AppClock {{ core: {}, app: {}, layout: {}, huc3: {} }}",
                lit(core),
                lit(app_slug),
                layout(&who, build)?,
                huc3(&who, build)?
            ));
        }
    }
    table(
        w,
        "Apps' own builds of a core that lay its clock out differently, sorted by core and then app.",
        "APP_CLOCKS",
        "crate::AppClock",
        &rows,
        Clone::clone,
    );
    Ok(())
}

fn core_kind(kind: &str) -> Result<&'static str, String> {
    match kind {
        "libretro" => Ok("Libretro"),
        "mister" => Ok("MiSTer"),
        "openfpga" => Ok("OpenFpga"),
        "standalone" => Ok("Standalone"),
        other => Err(format!("{other:?} is not a core kind")),
    }
}

/// The `AppCores` expression for one app's **Cores** column.
///
/// The column holds a core kind, for an app that loads every core of it, the slugs of the cores
/// it ships builds of, or both. Anything else — two kinds, a slug nobody listed, an empty cell — is
/// a registry change this generator has not been taught, and is refused rather than guessed at.
fn app_cores(app: &App, data: &Registries) -> Result<String, String> {
    let refuse = || {
        format!("apps.md: {:?} runs {:?}, which is not one core kind and listed cores", app.slug, app.cores)
    };
    let (kinds, builds): (Vec<&String>, Vec<&String>) =
        app.cores.iter().partition(|entry| core_kind(entry).is_ok());
    let kind = match kinds.as_slice() {
        [] if !builds.is_empty() => "None".to_owned(),
        [kind] => format!("Some(CoreKind::{})", core_kind(kind)?),
        _ => return Err(refuse()),
    };
    if !builds.iter().all(|slug| data.cores.iter().any(|c| &c.slug == *slug)) {
        return Err(refuse());
    }
    let builds: Vec<String> = builds.into_iter().cloned().collect();
    Ok(format!("AppCores {{ kind: {kind}, builds: {} }}", lits(&builds)))
}

/// Writes the generated tables, then formats them so the file is byte-stable.
///
/// Formatting here rather than leaving it to a later `cargo fmt` is what lets CI regenerate and
/// diff: without it the sync check and the format check would contradict each other.
///
/// The per-registry emitters are separate functions below; the seams are where one registry ends
/// and the next begins, which is the one place the data does have one.
pub fn write_rust(data: &Registries, clocks: &Clocks, layouts: &Layouts, path: &Path) -> Result<(), String> {
    // A clock layout naming a core that is not listed is a typo rather than a new core, and it
    // would otherwise vanish silently: nothing downstream reads this file by any other route.
    for slug in clocks.cores.keys() {
        if !data.cores.iter().any(|c| &c.slug == slug) {
            return Err(format!("clocks.json: {slug:?} is not a listed core"));
        }
    }

    check_layouts(data, layouts)?;

    // A core kind that is not one of the four is a registry change this generator has not been
    // taught, so it is checked before anything is written rather than producing a file that will
    // not compile.
    let kinds: Vec<&str> = data.cores.iter().map(|c| core_kind(&c.kind)).collect::<Result<_, _>>()?;

    let mut out = String::new();
    let w = &mut out;
    writeln!(w, "// @generated by `cargo xtask registries`. Do not edit by hand.").unwrap();
    writeln!(w, "//").unwrap();
    writeln!(w, "// The JSON under `data/` is itself synced from the registry pages at docs.1retro.com,")
        .unwrap();
    writeln!(w, "// so a change starts there, lands in the JSON, and reaches this file last.").unwrap();
    writeln!(w).unwrap();
    writeln!(
        w,
        "use crate::{{App, Binding, CardFormat, Core, DeviceKind, Platform, Role, RolePrefix, System, Vendor}};"
    )
    .unwrap();
    writeln!(w).unwrap();

    emit_systems(w, data)?;
    emit_cores(w, data, clocks, layouts, kinds)?;
    emit_apps(w, data)?;
    emit_app_clocks(w, data, clocks)?;
    emit_platforms(w, data)?;
    table(
        w,
        "Registered save roles that are compared whole, sorted by name.",
        "ROLES",
        "Role",
        &data.roles.roles,
        |r| {
            format!(
                "Role {{ name: {}, description: {}, common: {}, systems: {} }}",
                lit(&r.role),
                lit(&r.description),
                r.common,
                lits(&r.systems)
            )
        },
    );

    table(
        w,
        "Numbered-socket prefixes, sorted by prefix.",
        "ROLE_PREFIXES",
        "RolePrefix",
        &data.roles.prefixes,
        |p| {
            format!(
                "RolePrefix {{ prefix: {}, description: {}, systems: {} }}",
                lit(&p.prefix),
                lit(&p.description),
                lits(&p.systems)
            )
        },
    );

    table(
        w,
        "Card formats, in the order the Memory Cards specification lists them.",
        "CARD_FORMATS",
        "CardFormat",
        &data.card_formats,
        |f| {
            format!(
                "CardFormat {{ name: {}, dirent_len: {}, block_size: {}, notes: {} }}",
                lit(&f.format),
                f.dirent_len,
                f.block_size,
                lit(&f.notes)
            )
        },
    );

    table(
        w,
        "The spec-owned `device_kind` vocabulary.",
        "DEVICE_KINDS",
        "DeviceKind",
        &data.device_kinds,
        |k| format!("DeviceKind {{ name: {}, meaning: {} }}", lit(&k.value), lit(&k.meaning)),
    );

    table(w, "The spec-owned `binding` vocabulary.", "BINDINGS", "Binding", &data.bindings, |b| {
        format!("Binding {{ name: {}, bound_to: {} }}", lit(&b.value), lit(&b.bound_to))
    });

    table(w, "Assigned names in the `x` tree, sorted by name.", "VENDORS", "Vendor", &data.vendors, |v| {
        let holder = if v.vendor.eq_ignore_ascii_case("none") {
            "None".to_owned()
        } else {
            format!("Some({})", lit(&v.vendor))
        };
        format!(
            "Vendor {{ name: {}, reserved: {}, holder: {holder}, notes: {} }}",
            lit(&v.name),
            v.kind.trim().eq_ignore_ascii_case("reserved"),
            lit(&v.notes)
        )
    });

    write_and_format(path, &out)
}

/// One const per system, then the table built out of them, so the two cannot drift.
fn emit_systems(w: &mut String, data: &Registries) -> Result<(), String> {
    let system_names = const_names(data.systems.iter().map(|s| s.slug.as_str()))?;
    writeln!(w, "/// A named const per listed system, for the consumers that know which one they mean.")
        .unwrap();
    writeln!(w, "pub mod systems {{").unwrap();
    writeln!(w, "    use crate::System;\n").unwrap();
    for (s, name) in data.systems.iter().zip(&system_names) {
        if s.aliases.is_empty() {
            writeln!(w, "    /// {}.", s.name).unwrap();
        } else {
            writeln!(w, "    /// {} — also seen as {}.", s.name, s.aliases.join(", ")).unwrap();
        }
        writeln!(
            w,
            "    pub const {name}: System = System {{ slug: {}, name: {}, aliases: {} }};",
            lit(&s.slug),
            lit(&s.name),
            lits(&s.aliases),
        )
        .unwrap();
    }
    writeln!(w, "}}\n").unwrap();

    let systems: Vec<_> = data.systems.iter().zip(&system_names).collect();
    table(
        w,
        "Every system slug, sorted by slug so lookups can binary-search.",
        "SYSTEMS",
        "System",
        &systems,
        |(_, name)| format!("systems::{name}"),
    );

    Ok(())
}

/// The same shape as the systems above: a const each, then the table built out of them. It
/// costs nothing — a const has no symbol of its own, so a consumer naming one materialises that
/// core alone and leaves the table dead.
fn emit_cores(
    w: &mut String,
    data: &Registries,
    clocks: &Clocks,
    layouts: &Layouts,
    kinds: Vec<&str>,
) -> Result<(), String> {
    // The same shape as the systems above: a const each, then the table built out of them. It
    // costs nothing — a const has no symbol of its own, so a consumer naming one materialises that
    // core alone and leaves the table dead.
    let core_names = const_names(data.cores.iter().map(|c| c.slug.as_str()))?;
    let cores: Vec<_> = data.cores.iter().zip(kinds).zip(&core_names).collect();
    writeln!(w, "/// A named const per listed core, so a producer that knows which core it is can say so")
        .unwrap();
    writeln!(w, "/// without a lookup — and without linking the whole table.").unwrap();
    writeln!(w, "pub mod cores {{").unwrap();
    writeln!(w, "    use crate::{{Core, CoreKind, ExpandedLength, SaveLayout, SystemLayout}};\n").unwrap();
    for ((c, kind), name) in &cores {
        let (clock, huc3) = clock(&c.slug, clocks)?;
        let layouts = save_layouts(&c.slug, layouts)?;
        writeln!(w, "    /// {} — {}.", c.name, c.systems.join(", ")).unwrap();
        writeln!(
            w,
            "    pub const {name}: Core = Core {{ slug: {}, name: {}, kind: CoreKind::{kind}, \
             systems: {}, aliases: {}, clock: {clock}, huc3: {huc3}, layouts: {layouts} }};",
            lit(&c.slug),
            lit(&c.name),
            lits(&c.systems),
            lits(&c.aliases),
        )
        .unwrap();
    }
    writeln!(w, "}}\n").unwrap();

    table(w, "Every known emulator core, sorted by slug.", "CORES", "Core", &cores, |(_, name)| {
        format!("cores::{name}")
    });

    Ok(())
}

/// Apps the same way as cores: a const each, then the table built out of them.
///
/// Like everything here it fails into the buffer rather than the file, so a refusal below still
/// leaves the committed tables untouched.
fn emit_apps(w: &mut String, data: &Registries) -> Result<(), String> {
    let runs: Vec<String> = data.apps.iter().map(|a| app_cores(a, data)).collect::<Result<_, _>>()?;

    // A device whose firmware loads cores itself is the app, so the two pages are kept disjoint
    // by the specification. A name on both would give one producer two readings.
    for app in &data.apps {
        if data.platforms.iter().any(|p| p.slug == app.slug) {
            return Err(format!("{:?} is listed as both an app and a platform", app.slug));
        }
    }

    let app_names = const_names(data.apps.iter().map(|a| a.slug.as_str()))?;
    let apps: Vec<_> = data.apps.iter().zip(&runs).zip(&app_names).collect();
    // `CoreKind` is only named by an app that loads a whole kind, so it is only imported then.
    let kind_import = if runs.iter().any(|r| r.contains("CoreKind")) { ", CoreKind" } else { "" };
    writeln!(w, "/// A named const per listed app, for a producer that knows which one it is.").unwrap();
    writeln!(w, "pub mod apps {{").unwrap();
    writeln!(w, "    use crate::{{App, AppCores{kind_import}}};\n").unwrap();
    for ((a, runs), name) in &apps {
        writeln!(w, "    /// {}.", a.name).unwrap();
        writeln!(
            w,
            "    pub const {name}: App = App {{ slug: {}, name: {}, cores: {runs} }};",
            lit(&a.slug),
            lit(&a.name),
        )
        .unwrap();
    }
    writeln!(w, "}}\n").unwrap();

    table(w, "Every known app, sorted by slug.", "APPS", "App", &apps, |(_, name)| format!("apps::{name}"));

    Ok(())
}

/// Platforms the same way again.
fn emit_platforms(w: &mut String, data: &Registries) -> Result<(), String> {
    let platform_names = const_names(data.platforms.iter().map(|p| p.slug.as_str()))?;
    writeln!(w, "/// A named const per listed platform, for a producer that knows which one it runs on.")
        .unwrap();
    writeln!(w, "pub mod platforms {{").unwrap();
    writeln!(w, "    use crate::Platform;\n").unwrap();
    for (p, name) in data.platforms.iter().zip(&platform_names) {
        writeln!(w, "    /// {}.", p.name).unwrap();
        writeln!(
            w,
            "    pub const {name}: Platform = Platform {{ slug: {}, name: {} }};",
            lit(&p.slug),
            lit(&p.name),
        )
        .unwrap();
    }
    writeln!(w, "}}\n").unwrap();

    let platforms: Vec<_> = data.platforms.iter().zip(&platform_names).collect();
    table(w, "Every known platform, sorted by slug.", "PLATFORMS", "Platform", &platforms, |(_, name)| {
        format!("platforms::{name}")
    });

    Ok(())
}

/// Writes the file and runs rustfmt over it, so the sync check and the format check agree.
fn write_and_format(path: &Path, out: &str) -> Result<(), String> {
    // The last table is followed by a blank line from `table`; trim it so the file ends cleanly.
    let out = out.trim_end().to_owned() + "\n";
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))?;

    let status = std::process::Command::new("rustfmt")
        .args(["--edition", "2024"])
        .arg(path)
        .status()
        .map_err(|e| format!("running rustfmt: {e}"))?;
    if !status.success() {
        return Err(format!("rustfmt failed on {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `clocks.json` entry, as the file spells it.
    fn entry(json: &str) -> Clock {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn every_layout_admits_the_fields_it_has_and_no_others() {
        let layout_of = |json| layout("core", &entry(json));
        assert_eq!(
            layout_of(r#"{"layout": "appended", "timestamp_width": 8}"#).unwrap(),
            "Some(crate::ClockLayout::Appended { timestamp_width: 8 })"
        );
        assert_eq!(
            layout_of(
                r#"{"layout": "sidecar", "record": "origin", "timestamp_width": 4, "byte_order": "big"}"#
            )
            .unwrap(),
            "Some(crate::ClockLayout::Sidecar(crate::SidecarRecord::Origin { timestamp_width: 4, big_endian: true }))"
        );
        assert_eq!(layout_of(r#"{"huc3": {"layout": "none"}}"#).unwrap(), "None", "a HuC-3 alone");
        for wrong in [
            r#"{"layout": "sidecar"}"#,
            r#"{"layout": "sidecar", "record": "origin", "timestamp_width": 8}"#,
            r#"{"layout": "sidecar", "record": "origin", "timestamp_width": 6, "byte_order": "big"}"#,
            r#"{"layout": "sidecar", "record": "registers", "timestamp_width": 8, "byte_order": "big"}"#,
            r#"{"layout": "sidecar", "record": "sameboy", "timestamp_width": 8}"#,
            r#"{"layout": "sidecar", "record": "nonesuch"}"#,
            r#"{"layout": "appended", "record": "registers", "timestamp_width": 8}"#,
            r#"{"layout": "packed", "timestamp_width": 4}"#,
            r#"{"timestamp_width": 4}"#,
        ] {
            assert!(layout_of(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn every_huc3_layout_admits_the_fields_it_has_and_no_others() {
        let huc3_of = |json: &str| huc3("core", &entry(&format!(r#"{{"huc3": {json}}}"#)));
        assert_eq!(
            huc3_of(r#"{"layout": "packed", "reserved": 16}"#).unwrap(),
            "Some(crate::Huc3Layout::Packed { reserved: 16 })"
        );
        assert_eq!(
            huc3_of(r#"{"layout": "appended", "record": "memory"}"#).unwrap(),
            "Some(crate::Huc3Layout::Memory)"
        );
        assert_eq!(huc3_of(r#"{"layout": "none"}"#).unwrap(), "Some(crate::Huc3Layout::NotKept)");
        assert_eq!(huc3("core", &entry("{}")).unwrap(), "None");
        for wrong in [
            r#"{"layout": "packed"}"#,
            r#"{"layout": "packed", "reserved": 16, "timestamp_width": 4}"#,
            r#"{"layout": "sidecar", "record": "registers", "timestamp_width": 8}"#,
            r#"{"layout": "sidecar", "record": "origin", "timestamp_width": 8}"#,
            r#"{"layout": "appended"}"#,
            r#"{"layout": "none", "reserved": 1}"#,
            r#"{"layout": "none", "huc3": {"layout": "none"}}"#,
        ] {
            assert!(huc3_of(wrong).is_err(), "{wrong}");
        }
    }
}
