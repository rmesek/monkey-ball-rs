//! Expected on-disk layout of a content root, mirroring WebMonkeyBall
//! (`src/main.ts` stage loading, `src/constants.ts` `STAGE_BASE_PATHS`).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameSource {
    Smb1,
    Smb2,
    Mb2ws,
}

impl GameSource {
    /// Base directory relative to the content root (same names as the web build).
    pub fn base_path(self) -> &'static str {
        match self {
            GameSource::Smb1 => "smb1_content/test",
            GameSource::Smb2 => "smb2_content/test",
            GameSource::Mb2ws => "mb2ws_content/test",
        }
    }
}

/// Files the web build fetches for one stage, relative to the content root.
/// `bg_name` is the background (e.g. `bg_jun`); `naomi` adds the Naomi model set.
pub fn stage_files(source: GameSource, stage_id: u32, bg_name: &str, naomi: bool) -> Vec<String> {
    let base = source.base_path();
    let id = format!("{stage_id:03}");
    let mut files = vec![
        format!("{base}/st{id}/STAGE{id}.lz"),
        format!("{base}/st{id}/st{id}.gma"),
        format!("{base}/st{id}/st{id}.tpl"),
        format!("{base}/init/common.gma"),
        format!("{base}/init/common.tpl"),
        format!("{base}/init/common_p.lz"),
        format!("{base}/init/common.lz"),
        format!("{base}/bg/{bg_name}.gma"),
        format!("{base}/bg/{bg_name}.tpl"),
    ];
    if naomi {
        files.push(format!("{base}/st{id}/st{id}_p.lz"));
        files.push(format!("{base}/st{id}/st{id}.lz"));
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smb1_stage_1_paths() {
        let f = stage_files(GameSource::Smb1, 1, "bg_jun", false);
        assert!(f.contains(&"smb1_content/test/st001/STAGE001.lz".to_string()));
        assert!(f.contains(&"smb1_content/test/bg/bg_jun.tpl".to_string()));
        assert_eq!(f.len(), 9);
    }
}
