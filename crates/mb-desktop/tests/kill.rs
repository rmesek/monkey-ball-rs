//! Killing the desktop program must never damage what it keeps: the data directory holds the imported
//! game data and the previous complete save or the new one, and the next start reads them.
//!
//! The real binary is started on the user's disc image with its own data directory, killed (not asked to
//! exit) while a course is running, and started again without the image. Needs the disc image (`MB_DISC`,
//! or `Super Monkey Ball*.rvz|iso|gcm` next to the repository) and a machine that can open a window;
//! without either the test skips.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use mb_assets::gamepack::PackFile;
use mb_game::{FileStorage, Profile, SlotStorage};

fn find_disc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MB_DISC") {
        return Some(PathBuf::from(p));
    }
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    std::fs::read_dir(parent).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let image = [".rvz", ".iso", ".gcm"].iter().any(|e| name.ends_with(e));
        (name.starts_with("super monkey ball") && image).then(|| entry.path())
    })
}

#[test]
fn killing_the_program_mid_run_leaves_the_saves_and_the_game_data_intact() {
    let Some(disc) = find_disc() else {
        eprintln!("skipped: no disc image (set MB_DISC)");
        return;
    };
    let dir = std::env::temp_dir().join(format!("mb-kill-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let run = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_monkey-ball"));
        c.args(args).arg("--mute");
        c.env("MB_SAVE_DIR", &dir);
        c.stdout(Stdio::null()).stderr(Stdio::null());
        c
    };
    // A first, clean run with the image: proves the machine can run the game at all, and imports.
    let first = run(&["--exit-after", "1"])
        .arg(&disc)
        .status()
        .expect("start the program");
    if !first.success() {
        eprintln!("skipped: the game does not start here ({first})");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let pack = dir.join("game.pack");
    let imported = PackFile::open(&pack).expect("the game data was imported");
    let size = imported.len();
    assert!(size > 100 << 20, "{size} bytes");
    drop(imported);

    // A save with something to lose.
    let mut storage = SlotStorage::new(FileStorage::new(&dir));
    let mut profile = Profile::load(&mut storage).0;
    profile.settings.unlock_all = true;
    profile.settings.music_volume = 70;
    profile.save(&mut storage).unwrap();

    // Run a course from the stored data, without the image, and kill the process in the middle of it,
    // several times, at different moments.
    for wait_ms in [1500u64, 2500, 4000] {
        let mut child = run(&["--course", "beginner"])
            .spawn()
            .expect("start the program");
        std::thread::sleep(Duration::from_millis(wait_ms));
        assert!(
            child.try_wait().unwrap().is_none(),
            "the game ended by itself after {wait_ms} ms"
        );
        child.kill().expect("kill");
        child.wait().unwrap();

        let mut after = SlotStorage::new(FileStorage::new(&dir));
        let (loaded, warning) = Profile::load(&mut after);
        assert_eq!(warning, None, "killed after {wait_ms} ms");
        assert!(loaded.settings.unlock_all && loaded.settings.music_volume == 70);
        assert_eq!(PackFile::open(&pack).unwrap().len(), size);
    }

    // What versions up to 0.2.0 left here (the path of the image) is cleared away at the next start.
    std::fs::write(dir.join("last_disc"), b"C:/somewhere/image.rvz").unwrap();
    std::fs::write(dir.join("last_disc.a"), b"x").unwrap();
    // What an import cut short leaves behind is not taken for game data.
    std::fs::write(dir.join("game.pack.tmp"), b"MBPK half a pack").unwrap();
    let again = run(&["--exit-after", "1"]).status().unwrap();
    assert!(again.success(), "restart after the kills: {again}");
    // Naming the image again does not copy it again.
    let modified = std::fs::metadata(&pack).unwrap().modified().unwrap();
    let named = run(&["--exit-after", "1"]).arg(&disc).status().unwrap();
    assert!(named.success());
    assert_eq!(
        std::fs::metadata(&pack).unwrap().modified().unwrap(),
        modified
    );
    // Nothing but the pack and the slot files (and at most a stray temp file) is in the directory.
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let known = ["profile.a", "profile.b", "game.pack"];
        assert!(
            known.contains(&name.as_str()) || name.ends_with(".tmp"),
            "unexpected file {name}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
