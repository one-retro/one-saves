#!/usr/bin/env python3
"""Bootstrap extraction of the registry tables from the docs repo into JSON."""
import json, re, sys, pathlib

DOCS = pathlib.Path("/Users/hansl/Sources/hansl/docs.1retro.com/src/content/docs")
OUT = pathlib.Path("/Users/hansl/Sources/one-retro/one-saves/crates/one-saves-registry/data")
OUT.mkdir(parents=True, exist_ok=True)

CODE = re.compile(r"`([^`]+)`")
LINK = re.compile(r"\[([^\]]+)\]\([^)]*\)")


def prose(text):
    """A table cell as plain text: markdown links flattened, code spans unquoted."""
    return CODE.sub(r"\1", LINK.sub(r"\1", text)).strip()


def cells(line):
    parts = [c.strip() for c in line.strip().strip("|").split("|")]
    return parts


def tables(path):
    """Yields (heading, header_cells, [row_cells]) for each pipe table in a markdown file."""
    heading = None
    lines = path.read_text().splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith("#"):
            heading = line.lstrip("#").strip()
        if line.startswith("|") and i + 1 < len(lines) and set(lines[i + 1].strip()) <= set("|-: "):
            header = cells(line)
            rows = []
            i += 2
            while i < len(lines) and lines[i].startswith("|"):
                rows.append(cells(lines[i]))
                i += 1
            yield heading, header, rows
            continue
        i += 1


def codes(text):
    """Every `code-span` in a cell, in order."""
    return CODE.findall(text)


def one_code(text):
    found = codes(text)
    if len(found) != 1:
        raise ValueError(f"expected exactly one code span in {text!r}, got {found}")
    return found[0]


# ---- systems -------------------------------------------------------------
systems = []
for heading, header, rows in tables(DOCS / "registries/systems.md"):
    if header[0] != "Slug":
        continue
    for r in rows:
        systems.append({"slug": one_code(r[0]), "name": r[1], "aliases": codes(r[2])})

# ---- cores ---------------------------------------------------------------
cores = []
for heading, header, rows in tables(DOCS / "registries/cores.md"):
    if header[0] != "Slug":
        continue
    for r in rows:
        cores.append(
            {
                "slug": one_code(r[0]),
                "name": r[1],
                "kind": one_code(r[2]),
                "systems": codes(r[3]),
                "aliases": codes(r[4]) if len(r) > 4 else [],
                "family": heading,
            }
        )

# ---- roles ---------------------------------------------------------------
roles, prefixes = [], []
for heading, header, rows in tables(DOCS / "registries/roles.md"):
    if header[0] == "Role":
        for r in rows:
            name = one_code(r[0])
            entry = {"role": name, "description": prose(r[-1]), "section": heading}
            if len(r) == 3:
                entry["systems"] = codes(r[1])
            (prefixes if name.endswith("-") else roles).append(entry)
    elif header[0] == "Prefix":
        for r in rows:
            prefixes.append({"role": one_code(r[0]), "description": prose(r[1]), "section": heading})


def merge(entries, key):
    """Folds the per-system listings of one name into a single entry.

    A role is registered once and may then be listed again under each system that has that
    socket, so the sections are a browsing aid rather than a scope. A role in **Common** means
    the same thing everywhere, which is not the same as being listed for no system at all, so
    that distinction is kept rather than flattened into an empty list.
    """
    merged = {}
    for e in entries:
        got = merged.setdefault(
            e["role"], {key: e["role"], "description": e["description"], "common": False, "systems": []}
        )
        if e["section"] in ("Common", "Numbered sockets"):
            got["common"] = True
            got["description"] = e["description"]
        for s in e.get("systems", []):
            if s not in got["systems"]:
                got["systems"].append(s)
    return sorted(merged.values(), key=lambda e: e[key])


prefixes = merge(prefixes, "prefix")
roles = merge(roles, "role")

# ---- vendors -------------------------------------------------------------
vendors = []
for heading, header, rows in tables(DOCS / "registries/vendors.md"):
    if header[0] != "Name":
        continue
    for r in rows:
        vendors.append({"name": one_code(r[0]), "kind": r[1], "vendor": r[2], "notes": prose(r[3])})

# ---- card formats (dirent lengths live in the Memory Cards spec) ----------
card_formats = []
for heading, header, rows in tables(DOCS / "specifications/memory-cards.md"):
    if header[0] != "Format":
        continue
    for r in rows:
        dirent = r[1].strip()
        card_formats.append(
            {
                "format": one_code(r[0]),
                "dirent_len": None if dirent == "none" else int(dirent),
                "notes": prose(r[2]),
            }
        )

# ---- spec-owned vocabularies from the format spec ------------------------
spec = DOCS / "specifications/universal-saves-format.md"
device_kinds, bindings = [], []
for heading, header, rows in tables(spec):
    if header == ["Value", "Meaning"]:
        device_kinds = [{"value": one_code(r[0]), "meaning": prose(r[1])} for r in rows]
    elif header == ["Value", "Bound to"]:
        bindings = [{"value": one_code(r[0]), "bound_to": prose(r[1])} for r in rows]

for name, data in [
    ("systems", systems),
    ("cores", cores),
    ("roles", {"roles": roles, "prefixes": prefixes}),
    ("vendors", vendors),
    ("card-formats", card_formats),
    ("device-kinds", device_kinds),
    ("bindings", bindings),
]:
    (OUT / f"{name}.json").write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")
    n = len(data) if isinstance(data, list) else sum(len(v) for v in data.values())
    print(f"{name:14} {n:4} entries")
