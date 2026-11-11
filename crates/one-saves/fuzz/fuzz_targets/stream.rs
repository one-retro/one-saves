//! The streaming reader held to `from_slice`, over whatever libFuzzer finds.
//!
//! The contract is `tests/differential/mod.rs`'s, included from there so this and the test suite
//! cannot drift apart:
//!
//! - every input: the reader's verdict is `from_slice`'s, with every part's `verify` passing where
//!   it accepts, read whole and cut; and where both accept, the same header, parts and payloads,
//!   and `Done::content_hash` equal to `Bundle::content_hash`;
//! - every input that is a good bundle: broken in exactly one place, a sample of the ways
//!   `single_faults` knows, the reader gives the error `from_slice` gives.

#![no_main]

use libfuzzer_sys::fuzz_target;
use one_saves::Strictness;

#[path = "../../tests/differential/mod.rs"]
mod differential;

use differential::{Agreement, agree_cut, reference, single_faults};

fuzz_target!(|bytes: &[u8]| {
    let cut: &[usize] = &[bytes.len() % 61 + 1];
    for strictness in [Strictness::Decoder, Strictness::Schema] {
        // Panics on any disagreement about the verdict, and on accepting, on anything handed over
        // that `from_slice` did not read.
        agree_cut("the input", bytes, strictness, &[cut]);

        if reference(bytes, strictness).is_ok() {
            let faults = single_faults(bytes);
            let every = faults.len() / 16 + 1;
            for (fault, broken) in faults.iter().skip(bytes.len() % every).step_by(every) {
                if let Agreement::OtherError { reference, streamed } =
                    agree_cut(fault, broken, strictness, &[cut])
                {
                    panic!("{fault} ({strictness:?}): from_slice {reference:?}, reader {streamed:?}");
                }
            }
        }
    }
});
