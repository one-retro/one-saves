"""Generate crates/rom-id/src/nds/games.rs from SkyEmu's src/nds_rom_database.h."""
import re, struct, sys
src, out = sys.argv[1], sys.argv[2]
text = open(src).read()
licence = re.search(r'/\*(.*?)\*/', text, re.S).group(1)
rows = [(int(a, 16), int(c, 16)) for a, _, c in re.findall(r'\{0x([0-9A-F]+), 0x([0-9A-F]+), 0x([0-9A-F]+)\}', text)]
names = {0: 'Nothing', 1: 'Eeprom4k', 2: 'Eeprom64k', 3: 'Eeprom512k', 4: 'Eeprom1m', 5: 'Flash2m', 6: 'Flash4m',
         7: 'Flash8m', 8: 'Nand8m', 9: 'Nand16m', 10: 'Nand64m'}
games = []
skipped = 0
for code, kind in rows:
    if kind not in names:
        skipped += 1
        continue
    b = struct.pack('<I', code)
    if not all(0x20 < x < 0x7F for x in b):
        raise SystemExit(f'not a printable code: {b!r}')
    games.append((b.decode('ascii'), names[kind]))
games.sort()
assert len({g for g, _ in games}) == len(games)
lines = ['//! Every game SkyEmu knows the save memory of, by its game code, sorted for a binary search.',
         '//!',
         '//! Generated from SkyEmu adacd078 `src/nds_rom_database.h`, which the melonDS team relicensed from',
         "//! melonDS's GPL-licensed ROM list under the MIT licence for SkyEmu. Its notice, as it stands there:",
         '//!']
for l in licence.strip('\n').split('\n'):
    l = l.strip()
    lines.append('//!' + (' ' + l if l else ''))
lines += ['//!',
          f'//! {skipped} rows the database gives no save type it defines are left out, so a lookup of those games',
          '//! answers that nobody knows. Regenerate with the script kept beside this module, `games.py`.',
          '',
          'use super::Memory::{',
          '    self, Eeprom1m, Eeprom4k, Eeprom64k, Eeprom512k, Flash2m, Flash4m, Flash8m, Nand8m, Nand16m, Nand64m,',
          '    Nothing,',
          '};',
          '',
          '#[rustfmt::skip]',
          'pub(super) static GAMES: &[([u8; 4], Memory)] = &[']
lines += [f'    (*b"{g}", {m}),' for g, m in games]
lines += ['];', '']
open(out, 'w').write('\n'.join(lines))
print(len(games), 'games,', skipped, 'skipped')
