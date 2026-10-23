//! Just enough ISO 9660 to find one file in the root directory.
//!
//! PlayStation discs carry their serial in `SYSTEM.CNF`, a plain text file at
//! the root, so reaching it means walking the volume descriptor to the root
//! directory and reading one entry. That is all this module does: no Joliet, no
//! Rock Ridge, no subdirectories, no multi-extent files.

use crate::sector::Prefix;

/// Where the primary volume descriptor always sits.
const PVD_SECTOR: usize = 16;

/// Offset of the root directory's record inside the descriptor.
const ROOT_RECORD: usize = 156;

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Whether this dump carries an ISO 9660 filesystem.
#[must_use]
pub fn is_iso9660(prefix: &mut Prefix<'_>) -> bool {
    matches!(prefix.sector(PVD_SECTOR), Some(pvd) if pvd.starts_with(b"\x01CD001\x01"))
}

/// Read a named file from the root directory, up to one sector of it.
///
/// Comparison ignores the `;1` version suffix ISO 9660 appends, and is
/// case-insensitive, because a disc may spell it either way.
/// Where a file in the root directory lives: its extent and its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// First sector of the file's data.
    pub lba: u32,
    /// Length in bytes.
    pub size: u32,
}

/// Find a file in the root directory, without reading a byte of its contents.
///
/// This is what a presence check should use. A file's data can sit anywhere on
/// the disc — a Neo Geo CD `IPL.TXT` is commonly seventeen megabytes in — so
/// reading it to prove it exists would push far past what identification is
/// allowed to buffer, and fail on exactly the discs it was meant to recognise.
///
/// Comparison ignores the `;1` version suffix ISO 9660 appends, which some
/// discs write and others do not, and is case-insensitive.
#[must_use]
pub fn find_root_file(prefix: &mut Prefix<'_>, want: &str) -> Option<Entry> {
    let pvd = prefix.sector(PVD_SECTOR)?;
    let root = pvd.get(ROOT_RECORD..ROOT_RECORD + 34)?;
    let (Some(root_lba), Some(root_len)) = (le_u32(root, 2), le_u32(root, 10)) else {
        return None;
    };
    let (root_lba, root_len) = (root_lba as usize, root_len as usize);

    let sector_count = root_len.div_ceil(crate::sector::USER_DATA);
    for index in 0..sector_count {
        let Some(dir) = prefix.sector(root_lba + index) else {
            break;
        };
        let mut at = 0usize;
        while at < dir.len() {
            let Some(&len) = dir.get(at) else { break };
            let len = len as usize;
            // A zero length means the rest of this sector is padding: records
            // never straddle a sector boundary.
            if len == 0 {
                break;
            }
            let Some(record) = dir.get(at..at + len) else {
                break;
            };
            let Some(&name_len) = record.get(32) else {
                break;
            };
            let Some(name) = record.get(33..33 + name_len as usize) else {
                break;
            };
            let name = String::from_utf8_lossy(name);
            let name = name.split(';').next().unwrap_or(&name);
            if name.eq_ignore_ascii_case(want) {
                let (Some(lba), Some(size)) = (le_u32(record, 2), le_u32(record, 10)) else {
                    return None;
                };
                return Some(Entry { lba, size });
            }
            at += len;
        }
    }
    None
}

/// Read a file from the root directory, up to one sector of it.
///
/// `None` when the file is absent **or** when its data lies past the prefix
/// held, which is why a caller that only needs to know a file exists should ask
/// [`find_root_file`] instead.
#[must_use]
pub fn read_root_file<'a>(prefix: &mut Prefix<'a>, want: &str) -> Option<&'a [u8]> {
    let entry = find_root_file(prefix, want)?;
    let data = prefix.sector(entry.lba as usize)?;
    let size = (entry.size as usize).min(data.len());
    Some(&data[..size])
}

/// Pull a PlayStation serial out of a `SYSTEM.CNF`.
///
/// The file is `KEY = VALUE` lines. `BOOT` is a PlayStation disc and `BOOT2` a
/// PlayStation 2 one, which is the only thing that tells the two apart, and the
/// value is a path like `cdrom:\SLUS_123.45;1`. The printed serial replaces the
/// underscore with a hyphen and drops the dot, giving `SLUS-12345`.
#[must_use]
pub fn parse_system_cnf(text: &str) -> Option<(&'static str, String)> {
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let generation = match key.to_ascii_uppercase().as_str() {
            "BOOT" => "psx",
            "BOOT2" => "ps2",
            _ => continue,
        };
        if let Some(serial) = normalize(value) {
            return Some((generation, serial));
        }
    }
    None
}

/// `cdrom:\SLUS_123.45;1` becomes `SLUS-12345`.
fn normalize(value: &str) -> Option<String> {
    let value = value.trim();
    // Take what follows the last path separator, then drop the version suffix.
    let file = value.rsplit(['\\', '/', ':']).next()?.trim();
    let file = file.split(';').next()?.trim();
    if file.is_empty() {
        return None;
    }
    let serial: String = file
        .chars()
        .filter(|c| *c != '.')
        .map(|c| if c == '_' { '-' } else { c.to_ascii_uppercase() })
        .collect();
    // A serial is letters, a hyphen and digits; anything else is not one.
    let plausible = serial.len() >= 6
        && serial.contains('-')
        && serial.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    plausible.then_some(serial)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_boot_line_names_a_playstation_and_its_serial() {
        let cnf = "BOOT = cdrom:\\SLUS_007.77;1\nTCB = 4\nEVENT = 10\nSTACK = 801FFFF0\n";
        assert_eq!(parse_system_cnf(cnf), Some(("psx", "SLUS-00777".to_string())));
    }

    #[test]
    fn boot2_is_what_makes_it_a_playstation_2() {
        let cnf = "BOOT2 = cdrom0:\\SLUS_203.12;1\nVER=1.00\nVMODE=NTSC\n";
        assert_eq!(parse_system_cnf(cnf), Some(("ps2", "SLUS-20312".to_string())));
    }

    #[test]
    fn a_cnf_without_a_boot_line_yields_nothing() {
        assert_eq!(parse_system_cnf("VER = 1.00\nVMODE = NTSC\n"), None);
    }

    #[test]
    fn a_value_that_is_not_serial_shaped_is_refused() {
        // Better to return nothing than to record a filename as a serial.
        assert_eq!(parse_system_cnf("BOOT = cdrom:\\PSX.EXE;1"), None);
    }
}
