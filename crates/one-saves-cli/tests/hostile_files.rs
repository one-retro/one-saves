//! `1saves` run on files built to hurt whoever opens them.
//!
//! A bundle, a card or a save is whatever someone handed over, so every byte of one is the
//! author's to choose, and so is anything shipped beside it in an archive. Each case here is one a
//! security review found doing harm — writing outside the directory, driving the terminal, taking
//! the stack, waiting forever — run end to end through the binary, since what matters is what the
//! user sees happen.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use one_saves::{Bundle, HashValue, Header, Part, PartKind, Payload, Shape};

/// A directory of this test's own, empty.
fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("1saves-hostile-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

/// Runs `1saves` with these arguments, failing the test if it has not finished in a minute.
fn run(arguments: &[&std::ffi::OsStr]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_1saves"))
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(60) {
            child.kill().unwrap();
            panic!("1saves {arguments:?} was still running after a minute");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

fn run_on(command: &str, file: &Path) -> Output {
    run(&[command.as_ref(), file.as_os_str()])
}

/// Exited with the failure status, rather than succeeding or being killed by a signal.
fn failed(output: &Output) -> bool {
    output.status.code() == Some(1)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A collection holding one save, whose own part's digest does not match its bytes while every
/// digest around it does: what flipping a byte in the save and resealing the outer level makes.
fn resealed_collection() -> Vec<u8> {
    let mut save = Bundle::new(Header::default(), vec![Part::new(0, *b"SAVE")]);
    save.parts[0].sha256 = HashValue::sha256_of(b"EVAS");
    let nested = Part::new(0, save.to_vec().unwrap()).kind(PartKind::Bundle);
    Bundle::new(Header::new(Shape::Collection), vec![nested]).to_vec().unwrap()
}

#[test]
fn verify_finds_a_bad_digest_inside_a_nested_bundle() {
    let directory = scratch("verify-nested");
    let file = directory.join("card.1saves");
    std::fs::write(&file, resealed_collection()).unwrap();

    let output = run_on("verify", &file);
    assert!(failed(&output), "verify passed a bundle holding a save that does not match its digest");
    assert!(stderr(&output).contains("part 0 > part 0: sha256 does not match"), "{}", stderr(&output));
}

#[test]
fn extract_writes_nothing_it_did_not_check() {
    let directory = scratch("extract-unchecked");
    let file = directory.join("save.1saves");
    let mut bundle = Bundle::new(Header::default(), vec![Part::new(0, *b"SAVE")]);
    bundle.parts[0].sha256 = HashValue::sha256_of(b"EVAS");
    std::fs::write(&file, bundle.to_vec().unwrap()).unwrap();

    let output = run_on("extract", &file);
    assert!(failed(&output), "{}", stderr(&output));
    assert!(stderr(&output).contains("sha256 does not match"), "{}", stderr(&output));
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1, "something was written");
}

#[test]
fn a_part_claiming_a_terabyte_is_turned_down_without_being_inflated() {
    let directory = scratch("claim");
    let file = directory.join("bomb.1saves");
    let mut part = Part::new(0, *b"SAVE");
    part.payload = Payload::Compressed { bytes: b"not even zstd".to_vec(), size: 1 << 40 };
    std::fs::write(&file, Bundle::new(Header::default(), vec![part]).to_vec().unwrap()).unwrap();

    for command in ["verify", "extract"] {
        let output = run_on(command, &file);
        assert!(failed(&output), "{command}: {}", stderr(&output));
        assert!(
            stderr(&output).contains("not inflated, claiming 1099511627776 bytes"),
            "{command}: {}",
            stderr(&output)
        );
    }
    let output = run_on("hash", &file);
    assert!(String::from_utf8_lossy(&output.stdout).contains("content unavailable: its parts claim"));
}

#[test]
fn cbor_nested_without_end_is_an_error_and_not_an_abort() {
    let directory = scratch("deep");
    let file = directory.join("deep.1saves");
    let mut bytes = vec![0x81; 200_000];
    bytes.push(0x00);
    std::fs::write(&file, bytes).unwrap();

    for command in ["inspect", "verify", "hash", "extract"] {
        let output = run_on(command, &file);
        assert!(failed(&output), "{command} ended with {:?}: {}", output.status, stderr(&output));
        assert!(stderr(&output).contains("nests deeper than"), "{command}: {}", stderr(&output));
    }
}

#[test]
fn an_error_quoting_the_file_reaches_the_terminal_escaped() {
    // The one encoding is `zstd`, and an error naming any other quotes it. Spelled as an escape
    // sequence the same length, it would clear the screen.
    let mut part = Part::new(0, *b"SAVE");
    part.payload = Payload::Compressed { bytes: b"x".to_vec(), size: 4 };
    let mut bytes = Bundle::new(Header::default(), vec![part]).to_vec().unwrap();
    let at = bytes.windows(5).position(|window| window == b"\x64zstd").expect("the encoding is in there");
    bytes[at + 1..at + 5].copy_from_slice(b"\x1b[2J");

    let directory = scratch("escape");
    let file = directory.join("escape.1saves");
    std::fs::write(&file, bytes).unwrap();
    for command in ["inspect", "verify", "hash", "extract"] {
        let output = run_on(command, &file);
        assert!(failed(&output), "{command}: {}", stderr(&output));
        assert!(!output.stderr.contains(&0x1B), "{command} wrote a raw escape: {:?}", stderr(&output));
        assert!(stderr(&output).contains(r"\u{1b}[2J"), "{command}: {}", stderr(&output));
    }
}

#[cfg(unix)]
#[test]
fn extract_does_not_write_through_a_link_planted_beside_the_bundle() {
    // What an archive extracted with its links leaves: the bundle, and a link named for the file
    // `extract` writes by default, pointing outside the directory.
    let directory = scratch("extract-link");
    let outside = scratch("extract-link-outside");
    let file = directory.join("save.1saves");
    std::fs::write(&file, Bundle::new(Header::default(), vec![Part::new(0, *b"SAVE")]).to_vec().unwrap())
        .unwrap();
    let victim = outside.join("planted");
    std::os::unix::fs::symlink(&victim, directory.join("save.srm")).unwrap();

    let output = run_on("extract", &file);
    assert!(failed(&output), "{}", stderr(&output));
    assert!(stderr(&output).contains("already exists"), "{}", stderr(&output));
    assert!(!victim.exists(), "extract wrote through the link");

    let output = run(&["extract".as_ref(), file.as_os_str(), "--force".as_ref()]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!victim.exists(), "extract --force wrote through the link");
    assert_eq!(std::fs::read(directory.join("save.srm")).unwrap(), b"SAVE");
}

#[cfg(unix)]
#[test]
fn convert_does_not_wait_on_a_fifo_where_a_clock_would_be() {
    let directory = scratch("fifo-clock");
    let save = directory.join("game.sav");
    std::fs::write(&save, [0u8; 8192]).unwrap();
    let made = Command::new("mkfifo").arg(directory.join("game.rtc")).status().unwrap();
    assert!(made.success());

    let output = run(&["convert".as_ref(), save.as_os_str(), "--system".as_ref(), "gba".as_ref()]);
    assert!(failed(&output), "{}", stderr(&output));
    assert!(stderr(&output).contains("not a regular file"), "{}", stderr(&output));
    assert!(stderr(&output).contains("--rtc-file"), "{}", stderr(&output));
}
