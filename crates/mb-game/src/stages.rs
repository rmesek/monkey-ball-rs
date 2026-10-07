//! Stage id helpers (ported from `constants.ts`).

use crate::bg_table::STAGE_BG_NAMES;

/// Default stage time limit in frames (60 s at 60 Hz), `DEFAULT_STAGE_TIME`.
pub const DEFAULT_STAGE_TIME: u32 = 60 * 60;

pub const BONUS_STAGE_MIN_ID: u32 = 91;
pub const BONUS_STAGE_MAX_ID: u32 = 95;

/// Bonus stages (91..=95): falling out or running out of time does not cost a life and still advances.
pub fn is_bonus_stage(stage_id: u32) -> bool {
    (BONUS_STAGE_MIN_ID..=BONUS_STAGE_MAX_ID).contains(&stage_id)
}

/// Background model name (`bg_jun`, ...) of a stage, `None` for ids the reference has no entry for.
pub fn bg_name(stage_id: u32) -> Option<&'static str> {
    STAGE_BG_NAMES.get(stage_id as usize).copied().flatten()
}

/// `ST_005_SLOPES` -> 5 (0 when the name has no `ST_nnn`).
pub fn stage_id_from_name(name: &str) -> u32 {
    name.strip_prefix("ST_")
        .and_then(|r| r.get(..3))
        .and_then(|d| d.parse().ok())
        .unwrap_or(0)
}

/// `ST_005_SLOPES` -> `SLOPES`, `ST_014_NARROW_BRIDGE` -> `NARROW BRIDGE`.
pub fn stage_label_from_name(name: &str) -> String {
    match name.strip_prefix("ST_").and_then(|r| r.get(4..)) {
        Some(rest) => rest.replace('_', " "),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_labels() {
        assert_eq!(stage_id_from_name("ST_005_SLOPES"), 5);
        assert_eq!(stage_id_from_name("ST_113_DAA_LOO_MAA"), 113);
        assert_eq!(stage_id_from_name("junk"), 0);
        assert_eq!(
            stage_label_from_name("ST_014_NARROW_BRIDGE"),
            "NARROW BRIDGE"
        );
        assert_eq!(stage_label_from_name("junk"), "junk");
    }

    #[test]
    fn backgrounds_match_reference_table() {
        assert_eq!(bg_name(1), Some("bg_jun"));
        assert_eq!(bg_name(5), Some("bg_sun"));
        assert_eq!(bg_name(10), None);
        assert_eq!(bg_name(91), Some("bg_bns"));
        assert_eq!(bg_name(121), Some("bg_mst"));
        assert_eq!(bg_name(100_000), None);
    }

    #[test]
    fn bonus_range() {
        assert!(
            !is_bonus_stage(90) && is_bonus_stage(91) && is_bonus_stage(95) && !is_bonus_stage(96)
        );
    }
}
