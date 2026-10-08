//! Per-stage background / lighting parameters, ported from `noclip/SuperMonkeyBall/StageInfo.ts`
//! (`BgInfos` and `STAGE_LIST`, SMB1 stages only).

use mb_render::Lighting;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BgInfo {
    pub file_name: &'static str,
    pub clear_color: [f32; 4],
    pub ambient: [f32; 3],
    pub light_color: [f32; 3],
    pub light_rot_x: i32,
    pub light_rot_y: i32,
}

impl BgInfo {
    pub fn lighting(&self) -> Lighting {
        Lighting {
            ambient: self.ambient,
            light_color: self.light_color,
            light_rot_x: self.light_rot_x,
            light_rot_y: self.light_rot_y,
            specular_scale: self.light_color,
        }
    }
}

const fn rgb8(c: u32) -> [f32; 4] {
    [
        ((c >> 24) & 0xff) as f32 / 255.0,
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
    ]
}

const W: [f32; 3] = [1.0, 1.0, 1.0];
const DEFAULT_ROT: (i32, i32) = (8192, 24576);

const fn bg(
    file_name: &'static str,
    clear: u32,
    ambient: [f32; 3],
    light_color: [f32; 3],
    rot: (i32, i32),
) -> BgInfo {
    BgInfo {
        file_name,
        clear_color: rgb8(clear),
        ambient,
        light_color,
        light_rot_x: rot.0,
        light_rot_y: rot.1,
    }
}

pub const JUNGLE: BgInfo = bg("bg_jun", 0xffffffff, [0.6; 3], W, DEFAULT_ROT);
pub const WATER: BgInfo = bg(
    "bg_wat",
    0x000000ff,
    [0.28, 0.48, 0.63],
    [0.6, 0.85, 1.0],
    DEFAULT_ROT,
);
pub const NIGHT: BgInfo = bg("bg_nig", 0x000000ff, [0.4, 0.4, 0.7], W, DEFAULT_ROT);
pub const SUNSET: BgInfo = bg("bg_sun", 0x000000ff, [0.4, 0.4, 0.7], W, DEFAULT_ROT);
pub const SPACE: BgInfo = bg("bg_spa", 0x000000ff, [0.5, 0.45, 0.6], W, (29184, 17664));
pub const SAND: BgInfo = bg("bg_snd", 0xd8bc77ff, [0.45, 0.4, 0.25], W, (24576, 24576));
pub const ICE: BgInfo = bg("bg_ice", 0x000000ff, [0.55, 0.6, 0.85], W, DEFAULT_ROT);
pub const STORM: BgInfo = bg("bg_stm", 0x000000ff, [0.3, 0.3, 0.45], W, DEFAULT_ROT);
pub const BONUS: BgInfo = bg(
    "bg_bns",
    0x000000ff,
    [0.6, 0.7, 0.8],
    [0.8; 3],
    (-11776, 21888),
);
pub const TARGET: BgInfo = bg("bg_pil", 0xffffffff, [0.6; 3], W, DEFAULT_ROT);
pub const GOLF: BgInfo = bg("bg_gol", 0xffffffff, [0.6; 3], W, DEFAULT_ROT);
pub const BOWLING: BgInfo = bg("bg_bow", 0xffffffff, [0.6; 3], W, DEFAULT_ROT);
pub const MASTER: BgInfo = bg("bg_mst", 0xffffcdff, [0.6; 3], W, DEFAULT_ROT);
pub const ENDING: BgInfo = bg("bg_ending", 0xffffffff, [0.6; 3], W, DEFAULT_ROT);

/// Background of a stage, or `None` for ids the game does not offer (see
/// [`mb_game::stages::is_playable_stage`]). Which stage uses which background is decided in one place,
/// `mb_game::stages::bg_name` (the reference's `STAGE_BG_NAMES`); this module only adds how each
/// background is lit.
pub fn bg_for_stage(id: u32) -> Option<&'static BgInfo> {
    mb_game::stages::bg_name(id).and_then(bg_by_name)
}

/// Like [`bg_for_stage`] but falls back to the nearest lower listed stage (then the jungle). Only for the
/// developer stage viewer, which can open the arcade leftovers the game itself does not offer.
pub fn bg_for_stage_or_nearest(id: u32) -> &'static BgInfo {
    (1..=id).rev().find_map(bg_for_stage).unwrap_or(&JUNGLE)
}

/// Look a background up by file name (`bg_jun`, ...), for the `--bg` override.
pub fn bg_by_name(name: &str) -> Option<&'static BgInfo> {
    [
        &JUNGLE, &WATER, &NIGHT, &SUNSET, &SPACE, &SAND, &ICE, &STORM, &BONUS, &TARGET, &GOLF,
        &BOWLING, &MASTER, &ENDING,
    ]
    .into_iter()
    .find(|b| b.file_name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_stages_map_to_reference_backgrounds() {
        assert_eq!(bg_for_stage(1).unwrap().file_name, "bg_jun");
        assert_eq!(bg_for_stage(5).unwrap().file_name, "bg_sun");
        assert_eq!(bg_for_stage(21).unwrap().file_name, "bg_nig");
        assert_eq!(bg_for_stage(91).unwrap().file_name, "bg_bns");
        assert!(bg_for_stage(10).is_none());
    }

    /// One table decides the background of a stage (`mb_game`); every name in it must be one this module
    /// can light, or the stage would fall back to another backdrop without anyone noticing.
    #[test]
    fn every_background_the_game_names_is_known_here() {
        let mut playable = 0;
        for id in 0..=300 {
            match mb_game::stages::bg_name(id) {
                Some(name) => {
                    let bg = bg_for_stage(id).unwrap_or_else(|| panic!("stage {id}: {name}"));
                    assert_eq!(bg.file_name, name, "stage {id}");
                    assert!(mb_game::stages::is_playable_stage(id));
                    playable += 1;
                }
                None => {
                    assert!(bg_for_stage(id).is_none(), "stage {id}");
                    assert!(!mb_game::stages::is_playable_stage(id));
                }
            }
        }
        assert_eq!(playable, 168);
        // Ids the two former tables disagreed on: the game's own table has them.
        for (id, name) in [
            (146, "bg_spa"),
            (149, "bg_spa"),
            (156, "bg_pil"),
            (160, "bg_pil"),
            (199, "bg_jun"),
        ] {
            assert_eq!(bg_for_stage(id).unwrap().file_name, name);
        }
        for d in mb_game::Difficulty::ALL {
            for e in mb_game::course::stage_list(d) {
                assert!(bg_for_stage(e.id).is_some(), "{} stage {}", d.key(), e.id);
            }
        }
    }

    #[test]
    fn unlisted_stage_falls_back_to_lower_neighbour() {
        assert_eq!(bg_for_stage_or_nearest(10).file_name, "bg_sun");
    }

    #[test]
    fn clear_colour_decodes_rgba8() {
        assert_eq!(rgb8(0xffffffff), [1.0; 4]);
        assert_eq!(rgb8(0x000000ff), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn lookup_by_file_name() {
        assert_eq!(bg_by_name("bg_spa"), Some(&SPACE));
        assert!(bg_by_name("nope").is_none());
    }
}
