//! The game's own sound-request table, read from `main.dol`.
//!
//! Game code asks for a sound with a request id (`0x4` = "READY", `0x5011` = bumper, ...). The executable
//! resolves it through a table of 16-byte descriptors, big-endian:
//!
//! ```text
//! s32 musyx_id   MusyX SFX id passed to sndFXStartEx (-1 = alias of the previous real entry, "DMY_CODE");
//!                for group headers the group number, for STRM_* entries the stream index
//! u32 name       pointer to a C string ("SND_SED_NAR_READY", "SFXroll_basic_macro", "GRPnar", ...)
//! u8  category   8 = narrator, 15..=19 = character voices, ...
//! u8  _, u8 _, u8 flags
//! s16 request    the request id this entry answers to (-1 = none)
//! s16 _
//! ```
//!
//! The list ends with the entry whose `musyx_id` is -2. Resolution (function at `0x8002b360` in the USA
//! executable): take the low 11 bits of the request id, find the entry with that `request` value (for the
//! voice categories there is one per character, in the order boy, girl, baby, gorilla), and if it is an
//! alias walk back to the real entry. Bits 11..=17 of the id are a signed 7-bit velocity offset from 100;
//! narrator lines get +10 and a few MusyX ids get a fixed boost. MusyX turns velocity into amplitude as
//! `(velocity / 127)^2`.

/// One descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundDesc {
    pub musyx_id: i32,
    pub name: String,
    pub category: u8,
    pub flags: u8,
    pub request: i16,
}

/// A request resolved to something playable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedSound {
    /// Global MusyX SFX id (index into the `test/snd/mkb` banks).
    pub musyx_id: u16,
    /// MIDI-style velocity, 0..=127.
    pub velocity: u8,
}

impl ResolvedSound {
    /// Linear gain MusyX applies for this velocity.
    pub fn amplitude(&self) -> f32 {
        velocity_amplitude(self.velocity)
    }
}

/// MusyX's velocity curve: `(v / 127)^2` (its 128-entry table in the executable is exactly this).
pub fn velocity_amplitude(velocity: u8) -> f32 {
    let v = f32::from(velocity.min(127)) / 127.0;
    v * v
}

/// Velocity the game computes for a request id that resolves to `musyx_id` in `category`.
pub fn request_velocity(request_id: u32, musyx_id: u16, category: u8) -> u8 {
    let mut delta = ((request_id >> 11) & 0x7f) as i32;
    // The fixed boosts are added to the raw 7-bit field before it is read as signed.
    let boost = if category == 8 || category == 14 || category >= 15 {
        10
    } else {
        match musyx_id {
            0xb2 | 0xb8 | 0xb9 => 27,
            0xce | 0xcf => 10,
            _ => 0,
        }
    };
    delta = (delta + boost) & 0x7f;
    if delta > 0x3f {
        delta -= 0x80;
    }
    (100 + delta).clamp(0, 127) as u8
}

#[derive(Debug, Clone, Default)]
pub struct SoundTable {
    pub entries: Vec<SoundDesc>,
}

/// File offset of a virtual address, from the DOL section table (7 text + 11 data sections).
fn dol_offset(dol: &[u8], addr: u32) -> Option<usize> {
    let word = |o: usize| {
        dol.get(o..o + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    for i in 0..18 {
        let (off, base, size) = (word(i * 4)?, word(0x48 + i * 4)?, word(0x90 + i * 4)?);
        if size != 0 && addr >= base && addr - base < size {
            return Some((off + (addr - base)) as usize);
        }
    }
    None
}

fn dol_address(dol: &[u8], offset: usize) -> Option<u32> {
    let word = |o: usize| {
        dol.get(o..o + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    for i in 0..18 {
        let (off, base, size) = (word(i * 4)?, word(0x48 + i * 4)?, word(0x90 + i * 4)?);
        if size != 0 && offset >= off as usize && offset - (off as usize) < size as usize {
            return Some(base + (offset - off as usize) as u32);
        }
    }
    None
}

fn c_string(dol: &[u8], addr: u32) -> Option<String> {
    let start = dol_offset(dol, addr)?;
    let len = dol.get(start..)?.iter().take(64).position(|&b| b == 0)?;
    let bytes = &dol[start..start + len];
    (len > 0 && bytes.iter().all(|b| (0x20..0x7f).contains(b)))
        .then(|| String::from_utf8_lossy(bytes).into_owned())
}

/// Name of the table's second entry; unique in the executable, used to find the table.
const ANCHOR: &[u8] = b"SND_SEB_SE_COIN_SEPCM_2\0";
const ENTRY_LEN: usize = 16;
/// Far more than the 1061 entries of the USA table; stops a scan through garbage.
const MAX_ENTRIES: usize = 4096;

impl SoundTable {
    /// Locate and read the table. `None` when the executable has no such table (not SMB1, or a layout
    /// this reader does not know).
    pub fn from_dol(dol: &[u8]) -> Option<SoundTable> {
        let anchor = dol.windows(ANCHOR.len()).position(|w| w == ANCHOR)?;
        let anchor_addr = dol_address(dol, anchor)?.to_be_bytes();
        // The descriptor that points at the anchor string is entry 1; its name field is at +4.
        let name_field = (0..dol.len().saturating_sub(4))
            .step_by(4)
            .find(|&o| dol[o..o + 4] == anchor_addr)?;
        let start = name_field.checked_sub(4 + ENTRY_LEN)?;
        let mut entries = Vec::new();
        for i in 0..MAX_ENTRIES {
            let e = dol.get(start + i * ENTRY_LEN..start + (i + 1) * ENTRY_LEN)?;
            let musyx_id = i32::from_be_bytes([e[0], e[1], e[2], e[3]]);
            let name = c_string(dol, u32::from_be_bytes([e[4], e[5], e[6], e[7]]))?;
            entries.push(SoundDesc {
                musyx_id,
                name,
                category: e[8],
                flags: e[11],
                request: i16::from_be_bytes([e[12], e[13]]),
            });
            if musyx_id == -2 {
                return Some(SoundTable { entries });
            }
        }
        None
    }

    /// Index of the real (non-alias) entry at or before `index`.
    fn real(&self, mut index: usize) -> Option<&SoundDesc> {
        while self.entries.get(index)?.musyx_id & 0xffff == 0xffff {
            index = index.checked_sub(1)?;
        }
        self.entries.get(index)
    }

    /// Resolve a request id the way the game does. `character` (0 boy, 1 girl, 2 baby, 3 gorilla) picks
    /// among the per-character entries of voice requests and is ignored otherwise.
    pub fn resolve(&self, request_id: u32, character: usize) -> Option<ResolvedSound> {
        let want = (request_id & 0x7ff) as i16;
        let mut seen = 0;
        for (i, e) in self.entries.iter().enumerate() {
            if e.request != want {
                continue;
            }
            if e.category >= 15 {
                seen += 1;
                if seen != character + 1 {
                    continue;
                }
            }
            let real = self.real(i)?;
            let musyx_id = u16::try_from(real.musyx_id).ok()?;
            let mut velocity = request_velocity(request_id, musyx_id, e.category);
            // The gorilla's voice is 20 louder.
            if e.category >= 15 && seen == 4 {
                velocity = (velocity + 20).min(127);
            }
            return Some(ResolvedSound { musyx_id, velocity });
        }
        None
    }

    /// MusyX id of the entry called `name` (for sounds the game starts by table index, like the ball
    /// roll layers `SND_SE01_RUN01..03`).
    pub fn by_name(&self, name: &str) -> Option<u16> {
        let e = self.entries.iter().find(|e| e.name == name)?;
        u16::try_from(e.musyx_id).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny DOL: one data section at 0x8000_1000 holding strings and a four-entry table.
    fn fake_dol() -> Vec<u8> {
        let mut d = vec![0u8; 0x100];
        let base = 0x8000_1000u32;
        let mut body: Vec<u8> = Vec::new();
        let mut names = Vec::new();
        for n in [
            "GRPse04",
            "SND_SEB_SE_COIN_SEPCM_2",
            "DMY_CODE",
            "SND_SED_NAR_READY",
            "GRPend",
        ] {
            names.push(base + body.len() as u32);
            body.extend_from_slice(n.as_bytes());
            body.push(0);
        }
        while !body.len().is_multiple_of(4) {
            body.push(0);
        }
        let entry = |musyx: i32, name: u32, cat: u8, req: i16| {
            let mut e = Vec::new();
            e.extend_from_slice(&musyx.to_be_bytes());
            e.extend_from_slice(&name.to_be_bytes());
            e.extend_from_slice(&[cat, 0, 0, 0]);
            e.extend_from_slice(&req.to_be_bytes());
            e.extend_from_slice(&[0, 0]);
            e
        };
        body.extend(entry(3, names[0], 1, -1));
        body.extend(entry(209, names[1], 7, 0));
        body.extend(entry(-1, names[2], 7, 77));
        body.extend(entry(170, names[3], 8, 4));
        body.extend(entry(-2, names[4], 1, -1));
        // Section 7 (first data section): offset 0x100.
        d[7 * 4..7 * 4 + 4].copy_from_slice(&0x100u32.to_be_bytes());
        d[0x48 + 7 * 4..0x48 + 7 * 4 + 4].copy_from_slice(&base.to_be_bytes());
        d[0x90 + 7 * 4..0x90 + 7 * 4 + 4].copy_from_slice(&(body.len() as u32).to_be_bytes());
        d.extend(body);
        d
    }

    #[test]
    fn reads_the_table_and_resolves_requests() {
        let t = SoundTable::from_dol(&fake_dol()).expect("table");
        assert_eq!(t.entries.len(), 5);
        assert_eq!(t.entries[3].name, "SND_SED_NAR_READY");
        // Plain request: velocity 100, narrator +10.
        assert_eq!(
            t.resolve(0x4, 0),
            Some(ResolvedSound {
                musyx_id: 170,
                velocity: 110
            })
        );
        // An alias answers with the previous real entry; the id's velocity bits apply (0x2800 = +5).
        assert_eq!(
            t.resolve(0x2800 | 77, 0),
            Some(ResolvedSound {
                musyx_id: 209,
                velocity: 105
            })
        );
        assert_eq!(t.resolve(0x123, 0), None);
        assert_eq!(t.by_name("SND_SEB_SE_COIN_SEPCM_2"), Some(209));
        assert_eq!(t.by_name("GRPend"), None);
        assert!(SoundTable::from_dol(&[0u8; 0x200]).is_none());
    }

    #[test]
    fn velocity_follows_the_request_bits() {
        assert_eq!(request_velocity(0x6, 164, 0), 100);
        // 0x3d806: field 0x7b = -5.
        assert_eq!(request_velocity(0x3d806, 164, 0), 95);
        assert_eq!(request_velocity(0x5011, 165, 2), 110);
        assert_eq!(request_velocity(0x3b003, 206, 10), 100);
        // Fixed boosts by MusyX id (soft ball hits, banana pickups).
        assert_eq!(request_velocity(0x13, 0xb2, 3), 127);
        assert_eq!(request_velocity(0x2f, 0xb0, 3), 100);
        assert_eq!(request_velocity(0x3, 0xce, 10), 110);
        assert_eq!(velocity_amplitude(127), 1.0);
        assert!((velocity_amplitude(100) - 0.62).abs() < 1e-3);
        assert_eq!(velocity_amplitude(200), 1.0);
    }
}
