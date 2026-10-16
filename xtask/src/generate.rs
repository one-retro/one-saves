//! Turning the registry JSON into the static tables `one-saves-registry` ships.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::registry::{App, Clocks, Registries};

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

/// The `ClockLayout` expression for one core, or `None` where the file records nothing.
fn clock(slug: &str, clocks: &Clocks) -> Result<String, String> {
    let Some(entry) = clocks.cores.get(slug) else { return Ok("None".to_owned()) };
    match (entry.layout.as_str(), entry.reserved) {
        ("appended", None) => Ok("Some(ClockLayout::Appended)".to_owned()),
        ("sidecar", None) => Ok("Some(ClockLayout::Sidecar)".to_owned()),
        ("packed", Some(n)) => Ok(format!("Some(ClockLayout::Packed {{ reserved: {n} }})")),
        ("packed", None) => Err(format!("clocks.json: {slug:?} is packed but sets no reserved length")),
        (other, _) => Err(format!("clocks.json: {slug:?} has unknown layout {other:?}")),
    }
}

fn core_kind(kind: &str) -> Result<&'static str, String> {
    match kind {
        "libretro" => Ok("Libretro"),
        "mister" => Ok("MiSTer"),
        "openfpga" => Ok("OpenFpga"),
        other => Err(format!("{other:?} is not a core kind")),
    }
}

/// The `AppCores` expression for one app's **Cores** column.
///
/// The column holds a core kind, for an app that loads every core of it, or the slugs of the cores
/// an app runs alone. Anything else — two kinds, a kind beside a slug, a slug nobody listed — is a
/// registry change this generator has not been taught, and is refused rather than guessed at.
fn app_cores(app: &App, data: &Registries) -> Result<String, String> {
    let listed = |slug: &String| data.cores.iter().any(|c| &c.slug == slug);
    if let [only] = app.cores.as_slice()
        && let Ok(kind) = core_kind(only)
    {
        return Ok(format!("AppCores::Kind(CoreKind::{kind})"));
    }
    if !app.cores.is_empty() && app.cores.iter().all(listed) {
        return Ok(format!("AppCores::Only({})", lits(&app.cores)));
    }
    Err(format!(
        "apps.md: {:?} runs {:?}, which is neither one core kind nor a list of listed cores",
        app.slug, app.cores
    ))
}

/// Writes the generated tables, then formats them so the file is byte-stable.
///
/// Formatting here rather than leaving it to a later `cargo fmt` is what lets CI regenerate and
/// diff: without it the sync check and the format check would contradict each other.
///
/// The per-registry emitters are separate functions below; the seams are where one registry ends
/// and the next begins, which is the one place the data does have one.
pub fn write_rust(data: &Registries, clocks: &Clocks, path: &Path) -> Result<(), String> {
    // A clock layout naming a core that is not listed is a typo rather than a new core, and it
    // would otherwise vanish silently: nothing downstream reads this file by any other route.
    for slug in clocks.cores.keys() {
        if !data.cores.iter().any(|c| &c.slug == slug) {
            return Err(format!("clocks.json: {slug:?} is not a listed core"));
        }
    }

    // A core kind that is not one of the three is a registry change this generator has not been
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
    emit_cores(w, data, clocks, kinds)?;
    emit_apps(w, data)?;
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
fn emit_cores(w: &mut String, data: &Registries, clocks: &Clocks, kinds: Vec<&str>) -> Result<(), String> {
    // The same shape as the systems above: a const each, then the table built out of them. It
    // costs nothing — a const has no symbol of its own, so a consumer naming one materialises that
    // core alone and leaves the table dead.
    let core_names = const_names(data.cores.iter().map(|c| c.slug.as_str()))?;
    let cores: Vec<_> = data.cores.iter().zip(kinds).zip(&core_names).collect();
    writeln!(w, "/// A named const per listed core, so a producer that knows which core it is can say so")
        .unwrap();
    writeln!(w, "/// without a lookup — and without linking the whole table.").unwrap();
    writeln!(w, "pub mod cores {{").unwrap();
    writeln!(w, "    use crate::{{ClockLayout, Core, CoreKind}};\n").unwrap();
    for ((c, kind), name) in &cores {
        writeln!(w, "    /// {} — {}.", c.name, c.systems.join(", ")).unwrap();
        writeln!(
            w,
            "    pub const {name}: Core = Core {{ slug: {}, name: {}, kind: CoreKind::{kind}, \
             systems: {}, aliases: {}, clock: {} }};",
            lit(&c.slug),
            lit(&c.name),
            lits(&c.systems),
            lits(&c.aliases),
            clock(&c.slug, clocks)?,
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
