//! Which course stages have a mirror floor, decided from the stage files alone (`mb_app::mirror`): no GPU
//! is needed, only the developer's own disc image (`MB_DISC`, or `Super Monkey Ball*.rvz|iso|gcm` next to
//! the repository); without it the test skips.
//!
//! The list below is what the rule makes of the USA disc. Stage ids, counts and heights are not game
//! data. A change to the rule or its tolerances that changes a row is then a deliberate edit here.
//!
//! `MB_BUDGET_PRINT=1 cargo test -p mb-app --release --test mirrors -- --nocapture` prints the list as
//! measured instead of comparing it.

mod common;

use common::find_disc;

use mb_app::content::{course_stage_ids, open_disc_image};
use mb_app::mirror::{MirrorPlane, NoMirror, StageMirror, group_poses, stage_mirror};
use mb_app::stage_scene::StageAssets;
use mb_render::glam::{Mat4, Vec3};

/// A plane as text: where it is at rest and whether it rides on an anim group.
fn describe(plane: &MirrorPlane, rest: &[Mat4]) -> String {
    let m = plane.at(|i| rest.get(i).copied().unwrap_or(Mat4::IDENTITY));
    let origin = m.transform_point3(Vec3::ZERO);
    let normal = m.transform_vector3(Vec3::Y).normalize();
    let place = if normal.y > 0.99999 {
        format!("at y {:.3}", origin.y)
    } else {
        let lean = normal.y.clamp(-1.0, 1.0).acos().to_degrees();
        format!("leaning {lean:.1} deg through y {:.2}", origin.y / normal.y)
    };
    match plane {
        MirrorPlane::Fixed(_) => place,
        MirrorPlane::Group { group, .. } => format!("on group {group}, {place}"),
    }
}

/// `World.getMirrorPlaneMatrix` at rest with the camera above the stage, as text; `None` where the
/// reference ends up without a mirror.
fn reference_plane(a: &StageAssets, rest: &[Mat4]) -> Option<String> {
    let flat = |y: f32| Some(format!("at y {y:.3}"));
    match a.id {
        92 => flat(0.02),
        // Anim group 1's transform, which at rest is the identity: the plane y = 0.
        103 => rest.get(1).map(|m| {
            let m = MirrorPlane::Fixed(*m);
            format!("group 1's transform, {}", describe(&m, rest))
        }),
        110 => Some("at y -9.000 or y -43.000, by the camera".into()),
        48 | 102 => flat(0.0),
        _ => {
            let first = a.stagedef.mirror_names.first()?;
            let stage = a.stage_gma.as_ref().and_then(|(gma, _)| gma.model(first));
            let model = stage.or_else(|| a.backdrop.gma.model(first))?;
            flat(model.bound_sphere_center[1])
        }
    }
}

/// One line per stage whose stagedef names mirror models (and stage 92): what the port does, then what
/// the reference's `getMirrorPlaneMatrix` gives.
fn row(a: &StageAssets) -> Option<String> {
    let poses = group_poses(&a.stagedef);
    let rest = poses.first().cloned().unwrap_or_default();
    let reference = reference_plane(a, &rest).unwrap_or_else(|| "no mirror".into());
    let ours = match stage_mirror(a) {
        StageMirror::None(NoMirror::NotNamed) => return None,
        StageMirror::None(NoMirror::NoModel) => "none: no named model is in the archives".into(),
        StageMirror::None(NoMirror::NotFlat) => "none: no named model is flat".into(),
        StageMirror::Wavy(plane) => format!("wave floor, {}", describe(&plane, &rest)),
        StageMirror::Flat {
            entries,
            missing,
            plan,
        } => {
            let levels: Vec<String> = (plan.levels.iter())
                .map(|l| format!("{} {}", l.surfaces.len(), describe(&l.plane, &rest)))
                .collect();
            let groups: std::collections::BTreeSet<usize> =
                entries.iter().map(|e| e.group).collect();
            format!(
                "flat, {} named on {} group(s), {missing} missing, {} dropped: {}",
                entries.len(),
                groups.len(),
                plan.dropped.len(),
                levels.join(" + ")
            )
        }
    };
    Some(format!("{ours} | reference: {reference}"))
}

/// The mirror of every course stage that has one, in course order.
#[rustfmt::skip]
const MIRRORS: &[(u32, &str)] = &[
    (91, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.010 | reference: at y 0.010"),
    (92, "wave floor, at y 0.020 | reference: at y 0.020"),
    (21, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.015 | reference: at y 0.015"),
    (93, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.010 | reference: at y 0.010"),
    (48, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.015 | reference: at y 0.000"),
    (62, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 4.005 | reference: at y 4.005"),
    (94, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y -0.085 | reference: at y -0.085"),
    (71, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.010 | reference: at y 0.010"),
    (72, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.010 | reference: at y 0.010"),
    (74, "flat, 2 named on 1 group(s), 1 missing, 0 dropped: 1 at y 8.010 | reference: no mirror"),
    (75, "flat, 2 named on 1 group(s), 0 missing, 0 dropped: 2 at y 3.010 | reference: at y 3.010"),
    (76, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y -0.090 | reference: at y -0.090"),
    (77, "flat, 10 named on 1 group(s), 0 missing, 0 dropped: 10 at y 5.510 | reference: at y 5.510"),
    (78, "flat, 5 named on 5 group(s), 0 missing, 0 dropped: 5 at y -0.100 | reference: at y -0.100"),
    (95, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y -42.990 | reference: at y -42.990"),
    (82, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.015 | reference: at y 0.015"),
    (83, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y -0.085 | reference: at y -0.085"),
    (85, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.065 | reference: at y 0.065"),
    (86, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.015 | reference: at y 0.015"),
    (89, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.015 | reference: at y 0.015"),
    (101, "flat, 9 named on 9 group(s), 8 missing, 0 dropped: 1 at y 0.015 | reference: at y 0.015"),
    (102, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.015 | reference: at y 0.000"),
    (103, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 on group 1, leaning 9.0 deg through y 4.95 | reference: group 1's transform, at y 0.000"),
    (104, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.015 | reference: at y 0.015"),
    (105, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.010 | reference: at y 0.010"),
    (106, "flat, 16 named on 15 group(s), 0 missing, 0 dropped: 16 at y 0.010 | reference: at y 0.010"),
    (108, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 31.010 | reference: at y 31.010"),
    (109, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 8.015 | reference: at y 8.015"),
    (110, "flat, 2 named on 2 group(s), 0 missing, 0 dropped: 1 at y -8.990 + 1 at y -42.990 | reference: at y -9.000 or y -43.000, by the camera"),
    (111, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.040 | reference: at y 0.040"),
    (112, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 31.020 | reference: at y 31.020"),
    (113, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.010 | reference: at y 0.010"),
    (121, "flat, 24 named on 24 group(s), 0 missing, 0 dropped: 24 at y 1.000 | reference: at y 1.000"),
    (123, "flat, 21 named on 21 group(s), 0 missing, 0 dropped: 21 at y 0.000 | reference: at y 0.000"),
    (125, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.000 | reference: at y 0.000"),
    (128, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.000 | reference: at y 0.000"),
    (129, "flat, 1 named on 1 group(s), 0 missing, 0 dropped: 1 at y 0.005 | reference: at y 0.005"),
];

#[test]
fn every_course_stage_has_the_mirror_its_models_allow() {
    let Some(disc) = find_disc() else {
        eprintln!("skipped: no disc image (set MB_DISC)");
        return;
    };
    let content = match open_disc_image(&disc) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipped: {}: {e}", disc.display());
            return;
        }
    };
    let mut measured = Vec::new();
    let ids = course_stage_ids();
    for id in ids.iter().copied() {
        let a = StageAssets::load(&content, id, None).unwrap_or_else(|e| panic!("stage {id}: {e}"));
        if let Some(row) = row(&a) {
            measured.push((id, row));
        }
    }
    if std::env::var_os("MB_BUDGET_PRINT").is_some() {
        for (id, row) in &measured {
            println!("    ({id}, \"{row}\"),");
        }
        return;
    }
    let recorded: Vec<(u32, String)> = (MIRRORS.iter())
        .map(|(id, row)| (*id, row.to_string()))
        .collect();
    for (now, then) in measured.iter().zip(&recorded) {
        assert_eq!(now, then);
    }
    assert_eq!(measured.len(), recorded.len(), "{measured:#?}");
    // Every other course stage names no mirror model and has no mirror.
    assert_eq!(ids.len(), 109);
    let mirrors = measured.iter().filter(|(_, row)| !row.starts_with("none"));
    eprintln!(
        "{} of {} course stages have a mirror floor",
        mirrors.count(),
        ids.len()
    );
}
