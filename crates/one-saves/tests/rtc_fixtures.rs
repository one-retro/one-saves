//! The clock keys against the examples the specification tests its own schemas with.
//!
//! Every case is one from the specifications' `test/extensions/fixtures.js`, encoded as that
//! file's harness encodes it: a valid one has to read as its key's type and write back to the
//! same bytes, and an invalid one has to be refused. Re-encode the table when the clock pages
//! move; it is the pages' examples, not this crate's opinion of them.

use dcbor::CBOR;
use one_saves::extension::{
    Rtc, RtcHuc3, RtcJoybus, RtcMbc3, RtcRtc4513, RtcS3511a, RtcSrtc, RtcTc8521, RtcTpp1,
};

/// Key, whether the schema accepts it, the fixture's name, and its CBOR in hex.
const CASES: &[(&str, bool, &str, &str)] = &[
    ("x.1sav.rtc", true, "full", "a200c11a3e3b0e00011901f4"),
    ("x.1sav.rtc", true, "without-accuracy", "a100c11a3e3b0e00"),
    ("x.1sav.rtc", false, "without-reading", "a1011901f4"),
    ("x.1sav.rtc", false, "reading-as-float", "a100c1fb41cf1d8700400000"),
    ("x.1sav.rtc", false, "reading-untagged", "a1001a3e3b0e00"),
    ("x.1sav.rtc", false, "accuracy-negative", "a200c11a3e3b0e000120"),
    ("x.1sav.rtc", false, "source-clock-as-text", "a200c11a3e3b0e0001676762612d727463"),
    ("x.1sav.rtc", false, "unknown-key", "a200c11a3e3b0e0002646e6f7065"),
    ("x.1sav.rtc", false, "not-a-map", "6a31303434303537363030"),
    ("x.1sav.rtc.s3511a", true, "documented", "a2004701203f5e7d9cbb011840"),
    ("x.1sav.rtc.s3511a", true, "12-hour-mode", "a2004701203f5e7d9cbb0100"),
    ("x.1sav.rtc.s3511a", true, "with-alarm", "a4004701203f5e7d9cbb01184002c11a3e3b0e0003420080"),
    ("x.1sav.rtc.s3511a", true, "alarm-unknown", "a3004701203f5e7d9cbb01184003f6"),
    ("x.1sav.rtc.s3511a", false, "components-too-long", "a2004801203f5e7d9cbbda011840"),
    ("x.1sav.rtc.s3511a", false, "components-too-short", "a2004601203f5e7d9c011840"),
    ("x.1sav.rtc.s3511a", false, "without-components", "a1011840"),
    ("x.1sav.rtc.s3511a", false, "without-control", "a1004701203f5e7d9cbb"),
    ("x.1sav.rtc.s3511a", false, "hour24-as-its-own-key", "a3004701203f5e7d9cbb01184002f5"),
    ("x.1sav.rtc.s3511a", false, "components-as-array", "a2008700010203040506011840"),
    ("x.1sav.rtc.s3511a", false, "control-negative", "a2004701203f5e7d9cbb0120"),
    ("x.1sav.rtc.s3511a", false, "alarm-as-integer", "a3004701203f5e7d9cbb01184003198000"),
    ("x.1sav.rtc.s3511a", false, "alarm-too-long", "a3004701203f5e7d9cbb011840034301203f"),
    ("x.1sav.rtc.s3511a", false, "not-a-map", "623430"),
    ("x.1sav.rtc.mbc3", true, "documented", "a40085183b181e0c18c8010185183b181e0c18c80102c11a3e3b0e000304"),
    (
        "x.1sav.rtc.mbc3",
        true,
        "wide-timestamp",
        "a40085183b181e0c18c8010185183b181e0c18c80102c11a3e3b0e000308",
    ),
    (
        "x.1sav.rtc.mbc3",
        true,
        "latch-behind-live",
        "a40085183b181e0c18c80101850c181c0c18c80102c11a3e3b0e000304",
    ),
    (
        "x.1sav.rtc.mbc3",
        true,
        "halted-and-carried",
        "a4008500000018ff18c0018500000018ff18c002c11a3e3b0e000304",
    ),
    ("x.1sav.rtc.mbc3", true, "outside-a-footer", "a30085183b181e0c18c8010185183b181e0c18c80102c11a3e3b0e00"),
    ("x.1sav.rtc.mbc3", true, "latched-unknown", "a30085183b181e0c18c80101f602c11a3e3b0e00"),
    (
        "x.1sav.rtc.mbc3",
        false,
        "four-registers",
        "a40084183b181e0c18c80185183b181e0c18c80102c11a3e3b0e000304",
    ),
    (
        "x.1sav.rtc.mbc3",
        false,
        "six-registers",
        "a40086183b181e0c18c801000185183b181e0c18c80102c11a3e3b0e000304",
    ),
    ("x.1sav.rtc.mbc3", false, "without-live", "a30185183b181e0c18c80102c11a3e3b0e000304"),
    ("x.1sav.rtc.mbc3", false, "without-latched", "a30085183b181e0c18c80102c11a3e3b0e000304"),
    ("x.1sav.rtc.mbc3", false, "live-null", "a400f60185183b181e0c18c80102c11a3e3b0e000304"),
    (
        "x.1sav.rtc.mbc3",
        false,
        "timestamp-untagged",
        "a40085183b181e0c18c8010185183b181e0c18c801021a3e3b0e000304",
    ),
    (
        "x.1sav.rtc.mbc3",
        false,
        "timestamp-width-six",
        "a40085183b181e0c18c8010185183b181e0c18c80102c11a3e3b0e000306",
    ),
    (
        "x.1sav.rtc.mbc3",
        false,
        "timestamp-width-null",
        "a40085183b181e0c18c8010185183b181e0c18c80102c11a3e3b0e0003f6",
    ),
    (
        "x.1sav.rtc.mbc3",
        false,
        "registers-as-bytes",
        "a4004501203f5e7d0185183b181e0c18c80102c11a3e3b0e000304",
    ),
    ("x.1sav.rtc.mbc3", false, "not-a-map", "6a31303434303537363030"),
    ("x.1sav.rtc.srtc", true, "documented", "a2008d05000003080100020c0609090501c11a3e3b0e00"),
    ("x.1sav.rtc.srtc", true, "twenty-first-century", "a2008d05000003080100020c06020a0401c11a3e3b0e00"),
    ("x.1sav.rtc.srtc", true, "without-anchor", "a1008d05000003080100020c06090905"),
    ("x.1sav.rtc.srtc", false, "twelve-registers", "a1008c05000003080100020c060909"),
    ("x.1sav.rtc.srtc", false, "fourteen-registers", "a1008e05000003080100020c0609090500"),
    ("x.1sav.rtc.srtc", false, "register-too-wide", "a1008d05000003080100021006090905"),
    ("x.1sav.rtc.srtc", false, "registers-as-bytes", "a1004d01203f5e7d9cbbdaf918375675"),
    ("x.1sav.rtc.srtc", false, "without-registers", "a101c11a3e3b0e00"),
    ("x.1sav.rtc.srtc", false, "anchor-untagged", "a2008d05000003080100020c06090905011a3e3b0e00"),
    ("x.1sav.rtc.srtc", false, "not-a-map", "8d05000003080100020c06090905"),
    ("x.1sav.rtc.rtc4513", true, "documented", "a2009000050905030209020201050905020f0401c11a3e3b0e00"),
    ("x.1sav.rtc.rtc4513", true, "lost-and-12-hour", "a10090000d0905010509020201050905020f00"),
    ("x.1sav.rtc.rtc4513", true, "calendar-off", "a100900005090503020f070f070f0f05000f04"),
    ("x.1sav.rtc.rtc4513", false, "fifteen-registers", "a1008f00050905030209020201050905020f"),
    ("x.1sav.rtc.rtc4513", false, "seventeen-registers", "a1009100050905030209020201050905020f0400"),
    ("x.1sav.rtc.rtc4513", false, "register-too-wide", "a1009000050905030209020201050905021004"),
    ("x.1sav.rtc.rtc4513", false, "registers-as-bytes", "a1005001203f5e7d9cbbdaf91837567594b3d2"),
    ("x.1sav.rtc.rtc4513", false, "without-registers", "a101c11a3e3b0e00"),
    ("x.1sav.rtc.rtc4513", false, "hour24-as-its-own-key", "a2009000050905030209020201050905020f0402f5"),
    (
        "x.1sav.rtc.rtc4513",
        false,
        "anchor-as-float",
        "a2009000050905030209020201050905020f0401c1fb41cf1d8700400000",
    ),
    ("x.1sav.rtc.rtc4513", false, "not-a-map", "9000050905030209020201050905020f04"),
    (
        "x.1sav.rtc.joybus",
        true,
        "documented",
        "a400480300000000000000014800000000000000000248563492140604010103c11a3e3b0e00",
    ),
    (
        "x.1sav.rtc.joybus",
        true,
        "without-anchor",
        "a3004803000000000000000148000000000000000002485634921406040101",
    ),
    (
        "x.1sav.rtc.joybus",
        false,
        "time-block-short",
        "a30048030000000000000001480000000000000000024756349214060401",
    ),
    ("x.1sav.rtc.joybus", false, "without-ram-block", "a20048030000000000000002485634921406040101"),
    (
        "x.1sav.rtc.joybus",
        false,
        "block-3-carried",
        "a400480300000000000000014800000000000000000248563492140604010104480000000000000000",
    ),
    (
        "x.1sav.rtc.joybus",
        false,
        "time-as-array",
        "a3004803000000000000000148000000000000000002881856183418921406040101",
    ),
    (
        "x.1sav.rtc.joybus",
        false,
        "anchor-untagged",
        "a4004803000000000000000148000000000000000002485634921406040101031a3e3b0e00",
    ),
    ("x.1sav.rtc.joybus", false, "not-a-map", "581801203f5e7d9cbbdaf91837567594b3d2f1102f4e6d8cabca"),
    (
        "x.1sav.rtc.huc3",
        true,
        "full",
        "a30099010001080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01c11a3e3b0e00021825",
    ),
    (
        "x.1sav.rtc.huc3",
        true,
        "counters-and-event",
        "a300990100f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f601080f060d040bf6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f60900070e050c03f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f601c11a3e3b0e2a02182a",
    ),
    (
        "x.1sav.rtc.huc3",
        true,
        "counters-and-seconds",
        "a300990100f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f601080f060d040bf6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f601c11a3e3b0e0002183b",
    ),
    (
        "x.1sav.rtc.huc3",
        true,
        "counters-unknown",
        "a200990100f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f60b02f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f602f6",
    ),
    (
        "x.1sav.rtc.huc3",
        false,
        "window-short",
        "a20098ff01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030200",
    ),
    (
        "x.1sav.rtc.huc3",
        false,
        "window-long",
        "a20099010101080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a010200",
    ),
    (
        "x.1sav.rtc.huc3",
        false,
        "nybble-too-wide",
        "a2009901001001080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030200",
    ),
    (
        "x.1sav.rtc.huc3",
        false,
        "window-as-bytes",
        "a200588001203f5e7d9cbbdaf91837567594b3d2f1102f4e6d8cabcae90827466584a3c2e1001f3e5d7c9bbad9f81736557493b2d1f00f2e4d6c8baac9e80726456483a2c1e0ff1e3d5c7b9ab9d8f71635547392b1d0ef0e2d4c6b8aa9c8e70625446382a1c0dffe1d3c5b7a99b8d7f61534537291b0cfee0d2c4b6a89a8c7e6052443620200",
    ),
    ("x.1sav.rtc.huc3", false, "without-window", "a201c11a3e3b0e000200"),
    (
        "x.1sav.rtc.huc3",
        false,
        "anchor-untagged",
        "a30099010001080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a011a3e3b0e000200",
    ),
    (
        "x.1sav.rtc.huc3",
        false,
        "without-seconds",
        "a20099010001080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01c11a3e3b0e00",
    ),
    (
        "x.1sav.rtc.huc3",
        false,
        "seconds-past-the-minute",
        "a30099010001080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01c11a3e3b0e0002183c",
    ),
    (
        "x.1sav.rtc.huc3",
        false,
        "not-a-map",
        "99010001080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a01080f060d040b020900070e050c030a",
    ),
    (
        "x.1sav.rtc.tc8521",
        true,
        "full",
        "a300848d01080f060d040b020900070e058d000000030701020202000102008d030a01080f060d040b020900078d040b020900070e050c030a0108010802c11a3e3b0e00",
    ),
    (
        "x.1sav.rtc.tc8521",
        true,
        "time-and-settings",
        "a300848d000305040701f60202000106098d0000f6f6f6f6f6f6f6000101008df6f6f6f6f6f6f6f6f6f6f6f6f68df6f6f6f6f6f6f6f6f6f6f6f6f601f602c11a3e3b0e00",
    ),
    (
        "x.1sav.rtc.tc8521",
        false,
        "three-blocks",
        "a200838d01080f060d040b020900070e058d000000030701020202000102008d030a01080f060d040b020900070108",
    ),
    (
        "x.1sav.rtc.tc8521",
        false,
        "block-with-common-registers",
        "a200849001080f060d040b020900070e050c030a8d000000030701020202000102008d030a01080f060d040b020900078d040b020900070e050c030a01080108",
    ),
    (
        "x.1sav.rtc.tc8521",
        false,
        "register-too-wide",
        "a200848d1001080f060d040b020900070e8d000000030701020202000102008d030a01080f060d040b020900078d040b020900070e050c030a01080108",
    ),
    (
        "x.1sav.rtc.tc8521",
        false,
        "without-mode",
        "a100848d01080f060d040b020900070e058d000000030701020202000102008d030a01080f060d040b020900078d040b020900070e050c030a0108",
    ),
    (
        "x.1sav.rtc.tc8521",
        false,
        "mode-too-wide",
        "a200848d01080f060d040b020900070e058d000000030701020202000102008d030a01080f060d040b020900078d040b020900070e050c030a01080110",
    ),
    ("x.1sav.rtc.tc8521", false, "without-blocks", "a10108"),
    (
        "x.1sav.rtc.tc8521",
        false,
        "block-null",
        "a200848d01080f060d040b020900070e05f68d030a01080f060d040b020900078d040b020900070e050c030a01080108",
    ),
    (
        "x.1sav.rtc.tc8521",
        false,
        "not-a-map",
        "848d01080f060d040b020900070e058d000000030701020202000102008d030a01080f060d040b020900078d040b020900070e050c030a0108",
    ),
    ("x.1sav.rtc.tpp1", true, "documented", "a40044026e1e0f0144026e1e0f020403c11a3e3b0e00"),
    ("x.1sav.rtc.tpp1", true, "latched-unknown", "a40044026e1e0f01f6020403c11a3e3b0e00"),
    ("x.1sav.rtc.tpp1", true, "stopped-and-overflowed", "a30044000000000144000000000208"),
    ("x.1sav.rtc.tpp1", false, "rumble-bits-set", "a30044026e1e0f01f60205"),
    ("x.1sav.rtc.tpp1", false, "registers-too-long", "a30045026e1e0f0001f60204"),
    ("x.1sav.rtc.tpp1", false, "registers-as-array", "a3008402186e181e0f01f60204"),
    ("x.1sav.rtc.tpp1", false, "without-latched", "a20044026e1e0f0204"),
    ("x.1sav.rtc.tpp1", false, "without-status", "a20044026e1e0f01f6"),
    ("x.1sav.rtc.tpp1", false, "live-null", "a300f60144026e1e0f0204"),
    ("x.1sav.rtc.tpp1", false, "anchor-untagged", "a40044026e1e0f01f60204031a3e3b0e00"),
    ("x.1sav.rtc.tpp1", false, "not-a-map", "44026e1e0f"),
];

/// The value read as its key's type and written back out, or `None` when the type refuses it.
fn reread(key: &str, value: CBOR) -> Option<CBOR> {
    fn through<T: TryFrom<CBOR> + Into<CBOR>>(value: CBOR) -> Option<CBOR> {
        T::try_from(value).ok().map(Into::into)
    }
    match key {
        "x.1sav.rtc" => through::<Rtc>(value),
        "x.1sav.rtc.huc3" => through::<RtcHuc3>(value),
        "x.1sav.rtc.joybus" => through::<RtcJoybus>(value),
        "x.1sav.rtc.mbc3" => through::<RtcMbc3>(value),
        "x.1sav.rtc.rtc4513" => through::<RtcRtc4513>(value),
        "x.1sav.rtc.s3511a" => through::<RtcS3511a>(value),
        "x.1sav.rtc.srtc" => through::<RtcSrtc>(value),
        "x.1sav.rtc.tc8521" => through::<RtcTc8521>(value),
        "x.1sav.rtc.tpp1" => through::<RtcTpp1>(value),
        _ => panic!("{key} has no type here"),
    }
}

#[test]
fn every_clock_key_reads_what_its_schema_accepts_and_nothing_else() {
    let mut failures = Vec::new();
    for &(key, valid, name, hex) in CASES {
        // dcbor refuses anything not in deterministic form, which is a refusal like any other.
        let value = CBOR::try_from_hex(hex).ok();
        let reread = value.clone().and_then(|value| reread(key, value));
        let failure = match (valid, value, reread) {
            (true, Some(value), Some(reread)) if reread == value => None,
            (true, _, Some(_)) => Some("read, but wrote back different bytes"),
            (true, _, None) => Some("refused a value its schema accepts"),
            (false, _, Some(_)) => Some("read a value its schema refuses"),
            (false, _, None) => None,
        };
        if let Some(failure) = failure {
            failures.push(format!("{key} {name}: {failure}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // A table this test failed to hold would otherwise pass by testing nothing.
    assert_eq!(CASES.len(), 102);
}
