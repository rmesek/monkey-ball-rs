//! Killing the desktop program must never damage what it keeps: the save directory holds the previous
//! complete state or the new one, and the next start reads it.
//!
//! The real binary is started on the user's disc image with its own save directory, killed (not asked to
//! exit) while a course is running, and started again. Needs the disc image (`MB_DISC`, or
//! `Super Monkey Ball*.rvz|iso|gcm` next to the repository) and a machine that can open a window; without
//! either the test skips.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use mb_game::{FileStorage, Profile, SlotStorage, Storage};

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
fn killing_the_program_mid_run_leaves_the_saves_intact() {
    let Some(disc) = find_disc() else {
        eprintln!("skipped: no disc image (set MB_DISC)");
        return;
    };
    let dir = std::env::temp_dir().join(format!("mb-kill-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let run = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mb-app"));
        c.arg(&disc).args(args).arg("--mute");
        c.env("MB_SAVE_DIR", &dir);
        c.stdout(Stdio::null()).stderr(Stdio::null());
        c
    };
    // A first, clean run: proves the machine can run the game at all, and remembers the image.
    let first = run(&["--exit-after", "1"]).status().expect("start mb-app");
    if !first.success() {
        eprintln!("skipped: the game does not start here ({first})");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let mut storage = SlotStorage::new(FileStorage::new(&dir));
    let remembered = storage
        .read("last_disc")
        .unwrap()
        .expect("image remembered");
    assert!(String::from_utf8(remembered).unwrap().contains("Monkey"));

    // A save with something to lose.
    let mut profile = Profile::load(&mut storage).0;
    profile.settings.unlock_all = true;
    profile.settings.music_volume = 70;
    profile.save(&mut storage).unwrap();

    // Run a course and kill the process in the middle of it, several times, at different moments.
    for wait_ms in [1500u64, 2500, 4000] {
        let mut child = run(&["--course", "beginner"])
            .spawn()
            .expect("start mb-app");
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
        assert!(after.read("last_disc").unwrap().is_some());
    }

    // And it starts again on what is there, without asking for anything.
    let again = run(&["--exit-after", "1"]).status().unwrap();
    assert!(again.success(), "restart after the kills: {again}");
    // Nothing but the slot files (and at most a stray temp file) is in the directory.
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let known = ["profile.a", "profile.b", "last_disc.a", "last_disc.b"];
        assert!(
            known.contains(&name.as_str()) || name.ends_with(".tmp"),
            "unexpected file {name}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
