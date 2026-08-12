//! Building a card image from scratch.
//!
//! Everything structural is regenerated: the allocation table and its two levels of indirection,
//! the directory tree, the superblock and the spare-area ECC. A save's own directory entry is
//! carried through verbatim apart from the two fields that name where it landed, since block
//! placement is the allocator's business and everything else in the entry is the game's.

use crate::{ALLOCATED, CHAIN_END, ENTRY, Error, PAGE, Result, SPARE, Save, SuperBlock, ecc, mode};

/// A card size, as the retail cards come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capacity {
    /// The standard Sony card: 8 MB of data, 8650752 bytes as a hardware dump.
    Mb8,
    /// A 16 MB third-party card.
    Mb16,
    /// A 32 MB third-party card.
    Mb32,
    /// A 64 MB third-party card.
    Mb64,
}

impl Capacity {
    /// How many bytes of data this card holds.
    #[must_use]
    pub fn bytes(self) -> usize {
        match self {
            Capacity::Mb8 => 8 * 1024 * 1024,
            Capacity::Mb16 => 16 * 1024 * 1024,
            Capacity::Mb32 => 32 * 1024 * 1024,
            Capacity::Mb64 => 64 * 1024 * 1024,
        }
    }

    /// The capacity matching a byte count, if one does.
    #[must_use]
    pub fn from_bytes(bytes: usize) -> Option<Self> {
        [Capacity::Mb8, Capacity::Mb16, Capacity::Mb32, Capacity::Mb64]
            .into_iter()
            .find(|capacity| capacity.bytes() == bytes)
    }
}

const PAGES_PER_CLUSTER: usize = 2;
const PAGES_PER_BLOCK: usize = 16;
const CLUSTER: usize = PAGE * PAGES_PER_CLUSTER;
/// How many 32-bit entries one cluster holds.
const PER_CLUSTER: usize = CLUSTER / 4;
/// Clusters the formatter leaves in front of the indirect FAT.
const RESERVED_HEAD: usize = 8;
/// Erase blocks reserved at the end of the card for the write-backup mechanism.
const RESERVED_BLOCKS: usize = 2;

/// Assembles a card image out of saves.
pub struct CardBuilder {
    capacity: Capacity,
    saves: Vec<Save>,
}

impl CardBuilder {
    /// Starts an empty, formatted card of the given size.
    #[must_use]
    pub fn new(capacity: Capacity) -> Self {
        CardBuilder { capacity, saves: Vec::new() }
    }

    /// Adds a save. Saves land on the card in the order they are added.
    pub fn add(&mut self, save: Save) -> &mut Self {
        self.saves.push(save);
        self
    }

    /// Builds the card as plain data pages, with no spare areas.
    ///
    /// This is the `.ps2` form most emulators use.
    pub fn build(&self) -> Result<Vec<u8>> {
        let geometry = Geometry::for_capacity(self.capacity);
        let mut card = vec![0u8; geometry.clusters_per_card * CLUSTER];

        // Every FAT entry starts free, and every unallocated cluster is erased to 0xFF the way a
        // formatter leaves it.
        let mut fat = vec![CHAIN_END; geometry.alloc_end];
        let mut allocator = Allocator { next: 0, limit: geometry.alloc_end };

        // The root directory comes first, so its cluster is 0 and the superblock can say so.
        let root_entries = 2 + self.saves.len();
        let root_clusters = clusters_for(root_entries * ENTRY);
        let root_start = allocator.take(root_clusters)?;
        debug_assert_eq!(root_start, 0, "the root directory is the first thing allocated");

        let mut root = Vec::with_capacity(root_entries * ENTRY);
        root.extend_from_slice(&dot_entry(".", root_entries, root_start));
        root.extend_from_slice(&dot_entry("..", 0, root_start));

        // Each save is a directory of its own, laid down after the root.
        let mut written: Vec<(usize, Vec<u8>)> = Vec::new();
        for save in &self.saves {
            let entries = 2 + save.files.len();
            let dir_clusters = clusters_for(entries * ENTRY);
            let dir_start = allocator.take(dir_clusters)?;

            let mut directory = Vec::with_capacity(entries * ENTRY);
            directory.extend_from_slice(&dot_entry(".", entries, dir_start));
            directory.extend_from_slice(&dot_entry("..", 0, root_start));

            for file in &save.files {
                let file_clusters = clusters_for(file.data.len());
                let file_start = allocator.take(file_clusters)?;
                directory.extend_from_slice(&entry_for(
                    file.dirent.as_slice(),
                    &file.name,
                    mode::NEW_FILE,
                    file.data.len(),
                    file_start,
                ));
                written.push((file_start, file.data.clone()));
            }

            root.extend_from_slice(&entry_for(
                save.dirent.as_slice(),
                &save.name,
                mode::NEW_DIRECTORY,
                entries,
                dir_start,
            ));
            written.push((dir_start, directory));
        }
        written.push((root_start, root));

        // Lay every run of bytes down and chain the clusters it occupies.
        for (start, bytes) in written {
            let clusters = clusters_for(bytes.len()).max(1);
            for step in 0..clusters {
                let cluster = start + step;
                let from = step * CLUSTER;
                let to = ((step + 1) * CLUSTER).min(bytes.len());
                if from < bytes.len() {
                    let offset = (geometry.alloc_offset + cluster) * CLUSTER;
                    card[offset..offset + (to - from)].copy_from_slice(&bytes[from..to]);
                }
                fat[cluster] = if step + 1 == clusters {
                    ALLOCATED | CHAIN_END
                } else {
                    ALLOCATED | u32::try_from(cluster + 1).expect("cluster index fits")
                };
            }
        }

        geometry.superblock().write_into(&mut card);
        write_fat(&mut card, &geometry, &fat);
        Ok(card)
    }

    /// Builds the card as a hardware dump: every page followed by its 16-byte spare area.
    ///
    /// This is the `.ps2` form a physical reader produces, and it is longer than the card's
    /// capacity by design.
    pub fn build_with_spare(&self) -> Result<Vec<u8>> {
        let data = self.build()?;
        Ok(add_spare(&data))
    }
}

/// Interleaves spare areas into a plain image, computing each page's ECC.
#[must_use]
pub fn add_spare(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / PAGE * (PAGE + SPARE));
    for page in data.chunks_exact(PAGE) {
        out.extend_from_slice(page);
        let mut spare = [0u8; SPARE];
        ecc::fill_spare(page, &mut spare);
        out.extend_from_slice(&spare);
    }
    out
}

/// Where everything sits on a card of a given size.
struct Geometry {
    clusters_per_card: usize,
    alloc_offset: usize,
    alloc_end: usize,
    ifc_cluster: usize,
    fat_clusters: usize,
}

impl Geometry {
    /// Works out the layout a formatter would choose.
    ///
    /// The allocated area's size and the size of the table describing it depend on each other, so
    /// this settles by iteration rather than by a formula: each FAT cluster covers 256 clusters,
    /// and adding one pushes the allocated area's start along by one.
    fn for_capacity(capacity: Capacity) -> Self {
        let clusters_per_card = capacity.bytes() / CLUSTER;
        let reserved_tail = RESERVED_BLOCKS * (PAGES_PER_BLOCK / PAGES_PER_CLUSTER);
        let ifc_cluster = RESERVED_HEAD;

        let mut fat_clusters = 1;
        loop {
            let alloc_offset = ifc_cluster + 1 + fat_clusters;
            let alloc_end = clusters_per_card - alloc_offset - reserved_tail;
            let needed = alloc_end.div_ceil(PER_CLUSTER);
            if needed <= fat_clusters {
                return Geometry { clusters_per_card, alloc_offset, alloc_end, ifc_cluster, fat_clusters };
            }
            fat_clusters = needed;
        }
    }

    fn superblock(&self) -> SuperBlock {
        SuperBlock {
            version: "1.2.0.0".to_owned(),
            page_size: PAGE,
            pages_per_cluster: PAGES_PER_CLUSTER,
            pages_per_block: PAGES_PER_BLOCK,
            clusters_per_card: self.clusters_per_card,
            alloc_offset: self.alloc_offset,
            alloc_end: self.alloc_end,
            rootdir_cluster: 0,
            // One indirect FAT cluster covers 256 FAT clusters, which is far more than any card
            // this size needs, so the rest of the list stays clear.
            ifc_list: std::iter::once(u32::try_from(self.ifc_cluster).expect("indirect FAT cluster fits"))
                .chain(std::iter::repeat_n(0, 31))
                .collect(),
            backup_block1: u32::try_from(self.clusters_per_card / (PAGES_PER_BLOCK / PAGES_PER_CLUSTER) - 1)
                .expect("block index fits"),
            backup_block2: u32::try_from(self.clusters_per_card / (PAGES_PER_BLOCK / PAGES_PER_CLUSTER) - 2)
                .expect("block index fits"),
            card_type: 2,
            card_flags: 0x2B,
        }
    }
}

/// Writes the indirect FAT and the FAT clusters themselves.
fn write_fat(card: &mut [u8], geometry: &Geometry, fat: &[u32]) {
    let first_fat_cluster = geometry.ifc_cluster + 1;

    // The indirect FAT names each FAT cluster in turn.
    let ifc_offset = geometry.ifc_cluster * CLUSTER;
    for index in 0..geometry.fat_clusters {
        let value = u32::try_from(first_fat_cluster + index).expect("FAT cluster index fits");
        let at = ifc_offset + index * 4;
        card[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    // Then the entries, 256 to a cluster. Anything past the allocated area stays free.
    for (cluster, &entry) in fat.iter().enumerate() {
        let fat_cluster = first_fat_cluster + cluster / PER_CLUSTER;
        let at = fat_cluster * CLUSTER + (cluster % PER_CLUSTER) * 4;
        card[at..at + 4].copy_from_slice(&entry.to_le_bytes());
    }
}

/// Hands out runs of consecutive clusters.
struct Allocator {
    next: usize,
    limit: usize,
}

impl Allocator {
    fn take(&mut self, clusters: usize) -> Result<usize> {
        let clusters = clusters.max(1);
        if self.next + clusters > self.limit {
            return Err(Error::Full { needed: self.next + clusters, available: self.limit });
        }
        let start = self.next;
        self.next += clusters;
        Ok(start)
    }
}

fn clusters_for(bytes: usize) -> usize {
    bytes.div_ceil(CLUSTER).max(1)
}

/// Builds a `.` or `..` entry, which every directory opens with.
fn dot_entry(name: &str, length: usize, cluster: usize) -> Vec<u8> {
    let mut entry = vec![0u8; ENTRY];
    entry[..2].copy_from_slice(&mode::NEW_DIRECTORY.to_le_bytes());
    entry[4..8].copy_from_slice(&u32::try_from(length).expect("a length on this card fits").to_le_bytes());
    entry[16..20].copy_from_slice(&u32::try_from(cluster).expect("cluster index fits").to_le_bytes());
    let name = name.as_bytes();
    entry[0x40..0x40 + name.len()].copy_from_slice(name);
    entry
}

/// Builds a directory entry, keeping whatever the caller supplied and patching what moved.
///
/// The mode bits, timestamps and the rest of the entry belong to the game and come through
/// unchanged; only the length and the cluster are this writer's to set, because only those two
/// describe where the bytes landed.
fn entry_for(existing: &[u8], name: &str, default_mode: u16, length: usize, cluster: usize) -> Vec<u8> {
    let supplied_mode =
        if existing.len() >= ENTRY { u16::from_le_bytes([existing[0], existing[1]]) } else { 0 };

    // An entry that does not mark itself as existing is a placeholder rather than a game's own,
    // so there is nothing in it worth keeping. Trusting one verbatim would write a directory
    // entry the tree cannot find, which loses the save rather than preserving it.
    let mut entry = if supplied_mode & mode::EXISTS != 0 {
        let mut kept = existing[..ENTRY].to_vec();
        // The type bits are structural too: whether this is a file or a directory is decided by
        // what it holds, not by what an entry built elsewhere happened to say.
        let corrected = supplied_mode | (default_mode & (mode::FILE | mode::DIRECTORY));
        kept[..2].copy_from_slice(&corrected.to_le_bytes());
        kept
    } else {
        let mut fresh = vec![0u8; ENTRY];
        fresh[..2].copy_from_slice(&default_mode.to_le_bytes());
        let name = name.as_bytes();
        let len = name.len().min(32);
        fresh[0x40..0x40 + len].copy_from_slice(&name[..len]);
        fresh
    };
    entry[4..8].copy_from_slice(&u32::try_from(length).expect("a length on this card fits").to_le_bytes());
    entry[16..20].copy_from_slice(&u32::try_from(cluster).expect("cluster index fits").to_le_bytes());
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{File, MemoryCard};

    fn save(name: &str, files: &[(&str, usize)]) -> Save {
        Save {
            name: name.to_owned(),
            dirent: Vec::new(),
            files: files
                .iter()
                .map(|(file_name, len)| File {
                    name: (*file_name).to_owned(),
                    dirent: Vec::new(),
                    // Patterned rather than zeroed, so a run copied from the wrong cluster shows.
                    data: (0..*len).map(|i| u8::try_from(i % 251).expect("masked to a byte")).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn the_geometry_matches_a_retail_eight_megabyte_card() {
        // The layout a real Sony card is formatted to, which is the check that this is not just
        // internally consistent but right.
        let geometry = Geometry::for_capacity(Capacity::Mb8);
        assert_eq!(geometry.clusters_per_card, 8192);
        assert_eq!(geometry.alloc_offset, 41);
        assert_eq!(geometry.alloc_end, 8135);
        assert_eq!(geometry.ifc_cluster, 8);
        assert_eq!(geometry.fat_clusters, 32);
    }

    #[test]
    fn an_empty_card_parses_and_holds_nothing() {
        let card = CardBuilder::new(Capacity::Mb8).build().unwrap();
        let parsed = MemoryCard::parse(&card).unwrap();
        assert_eq!(parsed.saves().unwrap(), Vec::new());
        assert!(!parsed.had_spare());
    }

    #[test]
    fn a_save_survives_being_written_and_read_back() {
        // The worked example from the specification: a PS2 save is a directory of files.
        let mut builder = CardBuilder::new(Capacity::Mb8);
        builder.add(save("BASLUS-20312", &[("icon.sys", 964), ("list.ico", 15168), ("BASLUS-20312", 40960)]));
        let card = builder.build().unwrap();

        let saves = MemoryCard::parse(&card).unwrap().saves().unwrap();
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].name, "BASLUS-20312");
        assert_eq!(saves[0].files.len(), 3);
        assert_eq!(saves[0].files[0].name, "icon.sys");
        assert_eq!(saves[0].files[0].data.len(), 964);
        assert_eq!(saves[0].files[2].data.len(), 40960);
        assert_eq!(saves[0].len(), 964 + 15168 + 40960);
        // Every file's bytes, not just its length.
        assert_eq!(
            saves[0].files[1].data,
            (0..15168usize).map(|i| u8::try_from(i % 251).expect("masked")).collect::<Vec<u8>>()
        );
    }

    #[test]
    fn several_saves_round_trip_together() {
        let mut builder = CardBuilder::new(Capacity::Mb8);
        builder.add(save("BASLUS-20312", &[("icon.sys", 964), ("data.bin", 3000)]));
        builder.add(save("BASCUS-97129", &[("icon.sys", 964)]));
        builder.add(save("BESLES-50213", &[("a", 1), ("b", CLUSTER * 3), ("c", CLUSTER + 1)]));
        let card = builder.build().unwrap();

        let read_back = MemoryCard::parse(&card).unwrap().saves().unwrap();
        assert_eq!(read_back.len(), 3);
        for (original, got) in builder.saves.iter().zip(&read_back) {
            assert_eq!(original.name, got.name);
            assert_eq!(original.files.len(), got.files.len());
            for (a, b) in original.files.iter().zip(&got.files) {
                assert_eq!((&a.name, &a.data), (&b.name, &b.data), "{}/{}", original.name, a.name);
            }
        }
    }

    #[test]
    fn a_directory_entry_comes_back_verbatim_apart_from_where_it_landed() {
        // The mode bits and timestamps are the game's; only length and cluster are the writer's.
        let mut original = save("BASLUS-20312", &[("icon.sys", 100)]);
        let mut dirent = vec![0u8; ENTRY];
        dirent[..2].copy_from_slice(&mode::NEW_DIRECTORY.to_le_bytes());
        dirent[8..16].copy_from_slice(&[0x1E, 0x2D, 0x0C, 0x0B, 0x05, 0x00, 0xD4, 0x07]); // timestamp
        dirent[0x40..0x40 + 12].copy_from_slice(b"BASLUS-20312");
        original.dirent = dirent.clone();

        let mut builder = CardBuilder::new(Capacity::Mb8);
        builder.add(original);
        let card = builder.build().unwrap();
        let got = MemoryCard::parse(&card).unwrap().saves().unwrap().remove(0);

        assert_eq!(&got.dirent[8..16], &dirent[8..16], "the timestamp is the game's");
        assert_eq!(&got.dirent[..2], &dirent[..2], "so are the mode bits");
        assert_eq!(got.dirent.len(), ENTRY);
    }

    #[test]
    fn an_entry_off_a_real_card_keeps_its_mode_word_untouched() {
        // The writer forces the structural bits, which for an entry that came off a card is a
        // no-op: it already says it exists and already says which of the two it is. This is what
        // keeps "regenerate everything structural" from quietly rewriting a game's own entry.
        for (mode_word, is_directory) in
            [(mode::NEW_DIRECTORY, true), (mode::NEW_FILE, false), (0x8427 | 0x0008, true)]
        {
            let mut dirent = vec![0u8; ENTRY];
            dirent[..2].copy_from_slice(&mode_word.to_le_bytes());
            dirent[0x40..0x44].copy_from_slice(b"NAME");

            let default = if is_directory { mode::NEW_DIRECTORY } else { mode::NEW_FILE };
            let out = entry_for(&dirent, "NAME", default, 1, 0);
            assert_eq!(
                u16::from_le_bytes([out[0], out[1]]),
                mode_word,
                "mode {mode_word:#06x} should come through unchanged"
            );
        }
    }

    #[test]
    fn a_hardware_dump_carries_ecc_and_still_reads() {
        let mut builder = CardBuilder::new(Capacity::Mb8);
        builder.add(save("BASLUS-20312", &[("icon.sys", 964)]));
        let dump = builder.build_with_spare().unwrap();

        assert_eq!(dump.len(), 8_650_752);
        // The first page's spare area is its ECC, not padding.
        let mut expected = [0u8; SPARE];
        ecc::fill_spare(&dump[..PAGE], &mut expected);
        assert_eq!(&dump[PAGE..PAGE + SPARE], &expected);

        let parsed = MemoryCard::parse(&dump).unwrap();
        assert!(parsed.had_spare());
        assert_eq!(parsed.saves().unwrap()[0].files[0].data.len(), 964);
    }

    #[test]
    fn refuses_more_than_the_card_holds() {
        let mut builder = CardBuilder::new(Capacity::Mb8);
        for index in 0..9 {
            builder.add(save(&format!("SAVE-{index}"), &[("big", 1024 * 1024)]));
        }
        assert!(matches!(builder.build(), Err(Error::Full { .. })));
    }
}
