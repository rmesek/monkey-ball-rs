use super::*;

/// Tiny big-endian image builder.
struct Img(Vec<u8>);

impl Img {
    fn new(len: usize) -> Img {
        Img(vec![0; len])
    }
    fn u32(&mut self, o: usize, v: u32) {
        self.0[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
    fn u16(&mut self, o: usize, v: u16) {
        self.0[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }
    fn f32(&mut self, o: usize, v: f32) {
        self.u32(o, v.to_bits());
    }
    fn s(&mut self, o: usize, s: &str) {
        self.0[o..o + s.len()].copy_from_slice(s.as_bytes());
    }
}

/// One anim group at 0x100 with a 2x1 grid, 2 triangles, a goal, a keyframed track and a model name.
fn synthetic() -> Vec<u8> {
    let mut m = Img::new(0x800);
    // header
    m.u32(0x00, 0);
    m.u32(0x04, 600);
    m.u32(0x08, 1);
    m.u32(0x0c, 0x100);
    m.u32(0x10, 0x200); // start positions (2 of them, fallout follows)
    m.u32(0x14, 0x228);
    m.u32(0x18, 1);
    m.u32(0x1c, 0x240);
    m.u32(0x38, 1); // bananas
    m.u32(0x3c, 0x260);
    // start positions
    m.f32(0x200, 1.0);
    m.f32(0x204, 2.0);
    m.f32(0x208, 3.0);
    m.u16(0x20c, 0xfff0);
    m.f32(0x214 + 4, 5.0);
    // fallout
    m.f32(0x228, -20.0);
    // goal: pos, rot, type 'G' in low byte
    m.f32(0x240, 4.0);
    m.u16(0x24c, 0x10);
    m.u16(0x252, 0x0047);
    // banana
    m.f32(0x268, 9.0);
    m.u32(0x26c, 1);
    // anim group
    let g = 0x100;
    m.f32(g, 7.0);
    m.u16(g + 0x12, 2);
    m.u32(g + 0x14, 0x300); // anim header
    m.u32(g + 0x1c, 0x400); // tris
    m.u32(g + 0x20, 0x380); // grid table
    m.f32(g + 0x24, -10.0);
    m.f32(g + 0x28, -5.0);
    m.f32(g + 0x2c, 10.0);
    m.f32(g + 0x30, 20.0);
    m.u32(g + 0x34, 2);
    m.u32(g + 0x38, 1);
    m.u32(g + 0x3c, 1); // goals
    m.u32(g + 0x40, 0x240);
    m.u32(g + 0x80, 0x500); // models
    m.u32(g + 0x7c, 1);
    // anim header: posY has 2 keyframes
    m.u32(0x300 + 0x20, 2);
    m.u32(0x300 + 0x24, 0x340);
    m.u32(0x340, 1);
    m.f32(0x344, 0.5);
    m.f32(0x348, 12.0);
    m.f32(0x34c, 0.25);
    m.f32(0x350, 0.75);
    m.u32(0x354, 2);
    // grid: cell0 -> [0], cell1 NULL
    m.u32(0x380, 0x390);
    m.u16(0x390, 0);
    m.u16(0x392, 0xffff);
    // wait: second cell lists tri 1
    m.u32(0x384, 0x398);
    m.u16(0x398, 1);
    m.u16(0x39a, 0xffff);
    // triangles
    m.f32(0x400, 1.5);
    m.f32(0x400 + 0x0c + 4, 1.0);
    m.u16(0x400 + 0x1e, 0x8001);
    m.f32(0x400 + 0x20, 2.0);
    m.f32(0x400 + 0x38, 0.125);
    m.f32(0x440 + 0x28, 3.0);
    // model
    m.u32(0x500, 0x11);
    m.u32(0x504, 0x520);
    m.f32(0x508, 2.5);
    m.s(0x520, "MODEL_A");
    m.0
}

#[test]
fn decodes_synthetic_stage() {
    let s = parse_stagedef_smb1(&synthetic()).unwrap();
    assert_eq!(s.loop_end_seconds, 600);
    assert_eq!(s.start_positions.len(), 2);
    assert_eq!(
        s.start_positions[0].pos,
        Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0
        }
    );
    assert_eq!(s.start_positions[0].rot.x, -16);
    assert_eq!(s.start_positions[1].pos.y, 5.0);
    assert_eq!(s.fallout_y, -20.0);
    assert_eq!(s.goals.len(), 1);
    assert_eq!(s.goals[0].kind, GoalType::Green);
    assert_eq!(s.goals[0].rot.x, 0x10);
    assert_eq!(s.bananas[0].kind, 1);
    assert_eq!(s.bananas[0].pos.z, 9.0);

    let g = &s.anim_groups[0];
    assert_eq!(g.init_pos.x, 7.0);
    assert_eq!(g.anim_loop_type, 2);
    assert_eq!(g.goals.len(), 1);
    assert_eq!(g.grid.cells, vec![vec![0u16], vec![1u16]]);
    assert_eq!(g.triangles.len(), 2);
    assert_eq!(g.triangles[0].pos.x, 1.5);
    assert_eq!(g.triangles[0].normal.y, 1.0);
    assert_eq!(g.triangles[0].flags, 0x8001);
    assert_eq!(g.triangles[0].vert2.x, 2.0);
    assert_eq!(g.triangles[0].edge3_normal.x, 0.125);
    assert_eq!(g.triangles[1].vert3.x, 3.0);
    let a = g.anim.as_ref().unwrap();
    assert_eq!(a.pos_y.len(), 2);
    assert_eq!(a.pos_y[0].ease_type, 1);
    assert_eq!(a.pos_y[0].time_seconds, 0.5);
    assert_eq!(a.pos_y[0].value, 12.0);
    assert_eq!(a.pos_y[0].tangent_in, 0.25);
    assert_eq!(a.pos_y[0].tangent_out, 0.75);
    assert_eq!(a.pos_y[1].ease_type, 2);
    assert!(a.rot_x.is_empty());
    assert_eq!(g.models[0].name, "MODEL_A");
    assert_eq!(g.models[0].flags, 0x11);
    assert_eq!(g.models[0].value, 2.5);
}

#[test]
fn grid_lookup() {
    let s = parse_stagedef_smb1(&synthetic()).unwrap();
    let grid = &s.anim_groups[0].grid;
    assert_eq!(grid.cell_at(-5.0, 0.0), Some(&[0u16][..]));
    assert_eq!(grid.cell_at(5.0, 10.0), Some(&[1u16][..]));
    assert_eq!(grid.cell_at(15.0, 0.0), None);
    assert_eq!(grid.cell_at(-5.0, -6.0), None);
}

#[test]
fn goal_type_decoding() {
    assert_eq!(GoalType::from_value(0x0042), GoalType::Blue);
    assert_eq!(GoalType::from_value(0x4700), GoalType::Green);
    assert_eq!(GoalType::from_value(0x0052), GoalType::Red);
    assert_eq!(GoalType::from_value(0x0101), GoalType::Green);
    assert_eq!(GoalType::from_value(0x0201), GoalType::Red);
    assert_eq!(GoalType::from_value(0x0001), GoalType::Blue);
    assert_eq!(GoalType::from_value(0x0000), GoalType::Blue);
}

#[test]
fn smb2_magic_is_unsupported() {
    let mut m = Img::new(0x100);
    m.u32(4, 0x447a_0000);
    assert!(matches!(
        parse_stagedef_smb1(&m.0),
        Err(StageError::Unsupported(_))
    ));
    m.u32(4, 0x42c8_0000);
    assert!(matches!(
        parse_stagedef_smb1(&m.0),
        Err(StageError::Unsupported(_))
    ));
}

#[test]
fn rejects_truncated_and_bad_pointers() {
    assert!(matches!(
        parse_stagedef_smb1(&[0; 16]),
        Err(StageError::OutOfBounds { .. })
    ));
    let mut m = Img::new(0x100);
    m.u32(0x0c, 0x9000);
    assert!(matches!(
        parse_stagedef_smb1(&m.0),
        Err(StageError::BadPointer { at: 0x0c, .. })
    ));
    // list running past the end of the file
    let mut m = Img::new(0x100);
    m.u32(0x18, 100);
    m.u32(0x1c, 0x80);
    assert!(matches!(
        parse_stagedef_smb1(&m.0),
        Err(StageError::OutOfBounds { .. })
    ));
}

#[test]
fn empty_header_parses_to_empty_stage() {
    let s = parse_stagedef_smb1(&[0u8; 0xa0]).unwrap();
    assert!(s.anim_groups.is_empty() && s.goals.is_empty() && s.start_positions.is_empty());
}
