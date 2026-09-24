//! Writing a volume out of a set of saves.

use crate::{
    BackupRam, COMMENT_LEN, CONTINUATION_TAG, Container, ENTRY_LEN, Error, Geometry, MAGIC, NAME_LEN, Result,
    SAVE_TAG, Save, TAG_LEN, widen,
};

/// Builds a backup RAM image from saves.
///
/// Every save's entry and block list are written from scratch, because on this format they are
/// not separable from the payload: the list sits between the entry and the data, in the same run
/// of blocks, so the block numbers move whenever the save does. That is what the specification
/// means by Saturn being the one card format whose writer rewrites the payload rather than
/// treating it as opaque. The entry ahead of that list is not affected: [`Save::entry`] carries it
/// verbatim, and a save that brings one keeps its language byte and its fields' padding.
///
/// Blocks 0 and 1 are passed through from whatever [`system_area`](Self::system_area) was given,
/// so a volume round-trips rather than being reformatted.
#[derive(Debug, Clone)]
pub struct BackupBuilder {
    geometry: Geometry,
    /// Each save, and the blocks it was pinned to if a caller placed it.
    saves: Vec<(Save, Option<Vec<usize>>)>,
    system_area: Option<Vec<u8>>,
    container: Container,
}

impl BackupBuilder {
    /// A builder for an empty volume of this many bytes, allocating in blocks of `block`.
    ///
    /// Both are the caller's because nothing else settles them: the console's internal memory is
    /// 32 KiB in 64-byte blocks and a Backup RAM Cart is larger in larger ones, and a length on
    /// its own does not say which.
    pub fn new(size: usize, block: usize) -> Result<Self> {
        Ok(BackupBuilder {
            geometry: Geometry::new(size, block).ok_or(Error::WrongLength(size))?,
            saves: Vec::new(),
            system_area: None,
            container: Container::Packed,
        })
    }

    /// A builder set up to rebuild the volume this was read from, layout and all.
    ///
    /// Each save goes back on the blocks it came off, so a volume a console fragmented rebuilds
    /// as that console left it rather than being tidied into runs. Allocating afresh is what
    /// [`new`](Self::new) plus [`add`](Self::add) is for.
    #[must_use]
    pub fn from_volume(volume: &BackupRam) -> Self {
        BackupBuilder {
            geometry: volume.geometry(),
            saves: volume
                .saves()
                .iter()
                .cloned()
                .map(|save| {
                    let placed = (!save.blocks.is_empty()).then(|| save.blocks.clone());
                    (save, placed)
                })
                .collect(),
            system_area: Some(volume.system_area().to_vec()),
            container: volume.container(),
        }
    }

    /// Keeps the reserved blocks read off another volume: the signature and the block after it.
    ///
    /// A region that is not the right length for this geometry is ignored, since it is not this
    /// volume's reserved region.
    pub fn system_area(&mut self, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.system_area = Some(bytes.into());
        self
    }

    /// Writes the volume out in this container rather than packed.
    pub fn container(&mut self, container: Container) -> &mut Self {
        self.container = container;
        self
    }

    /// Adds a save. Its `block` is ignored: the builder allocates.
    pub fn add(&mut self, save: Save) -> &mut Self {
        self.saves.push((save, None));
        self
    }

    /// Adds a save on exactly these blocks, in this order.
    ///
    /// The allocator hands out runs, which is always a valid answer and never the only one: the
    /// block list exists so a save's blocks need not be adjacent, and a console that has had a
    /// save deleted out of the middle of its volume will fill the hole. This is how to write that
    /// layout deliberately — to reproduce a volume off a console that fragmented, or to build one
    /// for a reader to be tested against.
    ///
    /// The first block is where the entry goes. [`build`](Self::build) checks the run is long
    /// enough for the save and that no two saves overlap.
    pub fn add_at(&mut self, save: Save, blocks: impl Into<Vec<usize>>) -> &mut Self {
        self.saves.push((save, Some(blocks.into())));
        self
    }

    /// Writes the volume out, in whatever container was asked for.
    pub fn build(&self) -> Result<Vec<u8>> {
        let packed = self.build_packed()?;
        Ok(match self.container {
            Container::Packed => packed,
            Container::Wide => widen(&packed),
        })
    }

    /// Writes the volume's data, whatever container the result is going into.
    fn build_packed(&self) -> Result<Vec<u8>> {
        let g = self.geometry;
        let mut out = vec![0u8; g.size];
        match &self.system_area {
            Some(area) if area.len() == Geometry::FIRST_DATA_BLOCK * g.block => {
                out[..area.len()].copy_from_slice(area);
            }
            // A fresh volume is the signature over block 0 and zeros everywhere else, which is
            // what a console leaves: free blocks read back as zero on both real volumes here.
            _ => {
                for offset in (0..g.block).step_by(MAGIC.len()) {
                    out[offset..offset + MAGIC.len()].copy_from_slice(MAGIC);
                }
            }
        }

        // Blocks a placed save has claimed, so the allocator works around them and two placed
        // saves cannot be put on top of each other.
        let mut taken = vec![false; g.blocks];
        for slot in taken.iter_mut().take(Geometry::FIRST_DATA_BLOCK) {
            *slot = true;
        }
        for (save, placed) in &self.saves {
            let Some(blocks) = placed else { continue };
            for &block in blocks {
                if block >= g.blocks {
                    return Err(Error::Corrupt(format!(
                        "`{}` was placed on block {block}, past the volume's {}",
                        save.name, g.blocks
                    )));
                }
                if std::mem::replace(&mut taken[block], true) {
                    return Err(Error::Corrupt(format!(
                        "`{}` was placed on block {block}, which another save holds",
                        save.name
                    )));
                }
            }
        }

        let mut next = Geometry::FIRST_DATA_BLOCK;
        for (save, placed) in &self.saves {
            if save.data.is_empty() {
                return Err(Error::EmptySave(save.name.clone()));
            }
            if save.name.len() > NAME_LEN {
                return Err(Error::BadName(save.name.clone()));
            }
            let needed = g.blocks_for(save.data.len());

            let blocks = match placed {
                Some(blocks) if blocks.len() < needed => {
                    return Err(Error::Full { needed, available: blocks.len() });
                }
                Some(blocks) => blocks[..needed].to_vec(),
                // Nothing requires a run — the list is a list precisely so blocks need not be
                // adjacent — but a run is always a valid answer, and a reader follows the numbers
                // either way. Blocks a placed save claimed are stepped over.
                None => {
                    let mut run = Vec::with_capacity(needed);
                    while run.len() < needed {
                        if next >= g.blocks {
                            return Err(Error::Full {
                                needed,
                                available: taken.iter().filter(|t| !**t).count(),
                            });
                        }
                        if !taken[next] {
                            taken[next] = true;
                            run.push(next);
                        }
                        next += 1;
                    }
                    run
                }
            };
            write_save(&mut out, g, save, &blocks);
        }
        Ok(out)
    }
}

/// Lays one save down over the blocks it has been given.
fn write_save(out: &mut [u8], g: Geometry, save: &Save, blocks: &[usize]) {
    // The entry, then the list, then the data, as one continuous stream over the blocks'
    // content areas. Building it flat and cutting it up afterwards is what keeps the list's
    // spill into the second block from needing a case of its own.
    // A save read off a volume brings its entry with it, and none of what is in there depends on
    // where the save sits. Keeping it whole is what carries the language byte and the bytes a game
    // left in the padding of a field it did not fill — neither of which survives the fields above.
    let mut entry = if save.entry.len() == ENTRY_LEN - TAG_LEN {
        save.entry.clone()
    } else {
        let mut built = vec![0u8; ENTRY_LEN - TAG_LEN];
        built[..save.name.len()].copy_from_slice(save.name.as_bytes());
        built[0x0F - TAG_LEN] = save.language;
        let comment = save.comment.as_bytes();
        let comment = &comment[..comment.len().min(COMMENT_LEN)];
        built[0x10 - TAG_LEN..0x10 - TAG_LEN + comment.len()].copy_from_slice(comment);
        built[0x1A - TAG_LEN..0x1E - TAG_LEN].copy_from_slice(&save.date.to_be_bytes());
        built
    };
    // The length is the one field that has to agree with what is actually being written, so it is
    // rewritten either way: a caller that changed the payload must not leave the old figure.
    let size = u32::try_from(save.data.len()).expect("a save fits a volume, and a volume fits u32");
    entry[0x1E - TAG_LEN..0x22 - TAG_LEN].copy_from_slice(&size.to_be_bytes());

    let mut stream = entry;
    for &block in &blocks[1..] {
        let number = u16::try_from(block).expect("a block index fits a word");
        stream.extend_from_slice(&number.to_be_bytes());
    }
    stream.extend_from_slice(&0u16.to_be_bytes());
    stream.extend_from_slice(&save.data);

    for (index, &block) in blocks.iter().enumerate() {
        let at = block * g.block;
        out[at..at + TAG_LEN].copy_from_slice(if index == 0 { &SAVE_TAG } else { &CONTINUATION_TAG });
        let taken = index * g.content();
        let chunk = &stream[taken.min(stream.len())..(taken + g.content()).min(stream.len())];
        out[at + TAG_LEN..at + TAG_LEN + chunk.len()].copy_from_slice(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::formatted;

    fn save(name: &str, data: Vec<u8>) -> Save {
        Save {
            block: 0,
            blocks: Vec::new(),
            name: name.to_owned(),
            comment: "hello".to_owned(),
            language: 1,
            date: 24_576_100,
            data,
            entry: Vec::new(),
        }
    }

    #[test]
    fn a_volume_built_from_nothing_reads_back() {
        let mut builder = BackupBuilder::new(32_768, 64).expect("a real geometry");
        builder.add(save("PANDRA_3_01", vec![7u8; 1276]));
        let image = builder.build().expect("writes");

        let volume = BackupRam::parse(&image).expect("reads what it wrote");
        assert_eq!(volume.geometry().block, 64, "the signature states the block size");
        assert_eq!(volume.saves().len(), 1);
        let read = &volume.saves()[0];
        assert_eq!(read.name, "PANDRA_3_01");
        assert_eq!(read.comment, "hello");
        assert_eq!(read.language, 1);
        assert_eq!(read.data, vec![7u8; 1276]);
        assert_eq!(read.block, Geometry::FIRST_DATA_BLOCK, "the first save takes block 2");
    }

    #[test]
    fn a_save_that_outgrows_its_first_block_still_reads_back() {
        // The case that catches folding the 4-byte tag into the payload: a save of one block is
        // right either way, and every longer one is wrong if the tags are counted as data.
        let mut builder = BackupBuilder::new(32_768, 64).expect("a real geometry");
        let data: Vec<u8> = (0..4096).map(|b| u8::try_from(b & 0xFF).expect("a byte")).collect();
        builder.add(save("BIG", data.clone()));
        let volume = BackupRam::parse(&builder.build().expect("writes")).expect("reads");
        assert_eq!(volume.saves()[0].data, data);
    }

    #[test]
    fn two_saves_land_in_their_own_blocks_and_come_back_whole() {
        let mut builder = BackupBuilder::new(32_768, 64).expect("a real geometry");
        builder.add(save("FIRST", vec![1u8; 1276])).add(save("SECOND", vec![2u8; 300]));
        let volume = BackupRam::parse(&builder.build().expect("writes")).expect("reads");
        assert_eq!(volume.saves().len(), 2);
        assert_eq!(volume.saves()[0].data, vec![1u8; 1276]);
        assert_eq!(volume.saves()[1].data, vec![2u8; 300]);
        // 23 blocks for the first, so the second starts where it stops.
        assert_eq!(volume.saves()[1].block, Geometry::FIRST_DATA_BLOCK + 23);
    }

    /// A save's blocks are wherever its list leads, and nothing requires them to be adjacent.
    /// This builder hands out a run, so the list is rewritten by hand to scatter the blocks and
    /// check that the reader follows the numbers rather than walking forward from the first.
    #[test]
    fn a_block_list_is_followed_rather_than_assumed_to_be_a_run() {
        let mut builder = BackupBuilder::new(32_768, 64).expect("a real geometry");
        let data: Vec<u8> = (0..600).map(|b| u8::try_from(b & 0xFF).expect("a byte")).collect();
        builder.add(save("SCATTER", data.clone()));
        let mut image = builder.build().expect("writes");

        let g = Geometry::new(32_768, 64).expect("a real geometry");
        let listed = BackupRam::parse(&image).expect("reads").saves()[0].data.len();
        assert_eq!(listed, 600);

        // Move the save's last block out to block 100 and renumber the list's final entry. The
        // list lives in block 2's content, two bytes per block after the entry.
        let blocks = g.blocks_for(600);
        let last = Geometry::FIRST_DATA_BLOCK + blocks - 1;
        let (from, to) = (last * g.block, 100 * g.block);
        let moved: Vec<u8> = image[from..from + g.block].to_vec();
        image[to..to + g.block].copy_from_slice(&moved);
        image[from..from + g.block].fill(0);
        // The list names every block but the first, so the last one is entry `blocks - 2`.
        let slot = Geometry::FIRST_DATA_BLOCK * g.block + ENTRY_LEN + (blocks - 2) * 2;
        image[slot..slot + 2].copy_from_slice(&u16::try_from(100).expect("fits").to_be_bytes());

        let volume = BackupRam::parse(&image).expect("reads a scattered save");
        assert_eq!(volume.saves().len(), 1, "the moved block is not a save of its own");
        assert_eq!(volume.saves()[0].data, data, "followed the list rather than the run");
    }

    #[test]
    fn a_list_that_loops_is_refused_rather_than_read_forever() {
        let mut builder = BackupBuilder::new(32_768, 64).expect("a real geometry");
        builder.add(save("LOOP", vec![3u8; 600]));
        let mut image = builder.build().expect("writes");
        let g = Geometry::new(32_768, 64).expect("a real geometry");
        // Point the first list entry back at the save's own first block.
        let slot = Geometry::FIRST_DATA_BLOCK * g.block + ENTRY_LEN;
        let first = u16::try_from(Geometry::FIRST_DATA_BLOCK).expect("fits");
        image[slot..slot + 2].copy_from_slice(&first.to_be_bytes());
        assert!(matches!(BackupRam::parse(&image), Err(Error::Corrupt(_))));
    }

    #[test]
    fn refuses_what_will_not_fit_and_what_cannot_be_filed() {
        let mut full = BackupBuilder::new(32_768, 64).expect("a real geometry");
        full.add(save("HUGE", vec![0u8; 40_000]));
        assert!(matches!(full.build(), Err(Error::Full { .. })));

        let mut empty = BackupBuilder::new(32_768, 64).expect("a real geometry");
        empty.add(save("NOTHING", Vec::new()));
        assert!(matches!(empty.build(), Err(Error::EmptySave(_))));

        let mut named = BackupBuilder::new(32_768, 64).expect("a real geometry");
        named.add(save("THIS_NAME_IS_TOO_LONG", vec![1u8; 64]));
        assert!(matches!(named.build(), Err(Error::BadName(_))));

        assert!(matches!(BackupBuilder::new(100, 64), Err(Error::WrongLength(100))));
    }

    #[test]
    fn a_placed_save_must_fit_where_it_is_put() {
        // `add_at` is the caller doing the allocator's job, so the checks the allocator does for
        // itself have to be done on its behalf. A run too short would otherwise write a save's
        // tail over whatever came next.
        let mut short = BackupBuilder::new(32_768, 64).expect("a real geometry");
        short.add_at(save("SHORT", vec![1u8; 1276]), vec![2, 3, 4]);
        assert!(matches!(short.build(), Err(Error::Full { needed: 23, available: 3 })));

        let mut past = BackupBuilder::new(32_768, 64).expect("a real geometry");
        past.add_at(save("PAST", vec![1u8; 32]), vec![2, 9999]);
        let error = past.build().expect_err("block 9999 is not on a 512-block volume");
        assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
        assert!(error.to_string().contains("9999"), "{error}");
    }

    #[test]
    fn two_placed_saves_cannot_claim_the_same_block() {
        // The one mistake `add_at` makes easy, and the one whose result would look like a working
        // volume: the second save's bytes land on the first's blocks and both read back wrong.
        let mut builder = BackupBuilder::new(32_768, 64).expect("a real geometry");
        builder.add_at(save("FIRST", vec![1u8; 64]), vec![2, 3, 4]);
        builder.add_at(save("SECOND", vec![2u8; 64]), vec![4, 5, 6]);
        let error = builder.build().expect_err("block 4 is claimed twice");
        assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
        assert!(error.to_string().contains("another save holds"), "{error}");
    }

    #[test]
    fn the_allocator_steps_over_blocks_a_placed_save_claimed() {
        // Mixing the two: one save pinned, one left to the builder. The builder must not hand out
        // what the pinned one is sitting on, which is the case a volume with a hole in it needs.
        let mut builder = BackupBuilder::new(32_768, 64).expect("a real geometry");
        builder.add_at(save("PINNED", vec![7u8; 28]), vec![2]);
        builder.add(save("FLOATING", vec![8u8; 200]));
        let volume = BackupRam::parse(&builder.build().expect("writes")).expect("reads");

        let pinned = volume.saves().iter().find(|s| s.name == "PINNED").expect("there");
        let floating = volume.saves().iter().find(|s| s.name == "FLOATING").expect("there");
        assert_eq!(pinned.blocks, vec![2]);
        assert!(!floating.blocks.contains(&2), "the allocator stepped over it");
        assert_eq!(pinned.data, vec![7u8; 28]);
        assert_eq!(floating.data, vec![8u8; 200]);
    }

    #[test]
    fn a_save_that_overruns_the_blocks_it_names_is_refused() {
        // Two ways a volume can lie about itself, both of which would otherwise read past the end
        // of what the save actually holds.
        let mut builder = BackupBuilder::new(32_768, 64).expect("a real geometry");
        builder.add(save("HONEST", vec![5u8; 600]));
        let good = builder.build().expect("writes");
        let g = Geometry::new(32_768, 64).expect("a real geometry");

        // A length field far past what its blocks can hold.
        let mut lying = good.clone();
        let at = Geometry::FIRST_DATA_BLOCK * g.block + 0x1E;
        lying[at..at + 4].copy_from_slice(&30_000u32.to_be_bytes());
        let error = BackupRam::parse(&lying).expect_err("it cannot hold that");
        assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
        assert!(error.to_string().contains("30000"), "{error}");

        // A list entry pointing outside the volume.
        let mut astray = good;
        let list = Geometry::FIRST_DATA_BLOCK * g.block + ENTRY_LEN;
        astray[list..list + 2].copy_from_slice(&600u16.to_be_bytes());
        let error = BackupRam::parse(&astray).expect_err("block 600 is not on a 512-block volume");
        assert!(matches!(error, Error::Corrupt(_)), "{error:?}");
        assert!(error.to_string().contains("600"), "{error}");
    }

    #[test]
    fn a_formatted_volume_has_the_shape_a_console_leaves() {
        let built = BackupBuilder::new(32_768, 64).expect("a real geometry").build().expect("writes");
        assert_eq!(built, formatted(32_768, 64), "signature over block 0, zeros after it");
        assert!(BackupRam::parse(&built).expect("reads").saves().is_empty());
    }
}
