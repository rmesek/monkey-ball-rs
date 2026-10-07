// Generated from WebMonkeyBall src/course.ts (SMB1 scripts). Same command order as the reference.
use crate::course::Op::*;
use crate::course::Op;

pub(crate) const BEGINNER_MAIN: &[Op] = &[
    Floor("ST_001_PLAIN"), IfFloorClear, ThenJumpFloor(1), Floor("ST_002_DIAMOND"), IfFloorClear,
    IfGoalType(b'B'), ThenJumpFloor(1), IfFloorClear, IfGoalType(b'G'), ThenJumpFloor(3),
    Floor("ST_003_HAIRPIN"), IfFloorClear, ThenJumpFloor(1), Floor("ST_004_WIDE_BRIDGE"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_091_BONUS_BASIC"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_005_SLOPES"), IfFloorClear, ThenJumpFloor(1), Floor("ST_006_STEPS"), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_007_BLOCKS"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_008_JUMP_SINGLE"), IfFloorClear, ThenJumpFloor(1), Floor("ST_009_EXAM_A"),
    IfFloorClear, ThenOther, CourseEnd,
];

pub(crate) const ADVANCED_MAIN: &[Op] = &[
    Floor("ST_011_BUMP"), IfFloorClear, ThenJumpFloor(1), Floor("ST_012_WALKING"), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_013_REPULSE"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_014_NARROW_BRIDGE"), IfFloorClear, ThenJumpFloor(1), Floor("ST_091_BONUS_BASIC"),
    Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_015_BREAK"), IfFloorClear,
    IfGoalType(b'B'), ThenJumpFloor(1), IfFloorClear, IfGoalType(b'G'), ThenJumpFloor(4),
    Floor("ST_016_CURVES"), IfFloorClear, ThenJumpFloor(1), Floor("ST_017_DOWNHILL"), Time(1800),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_018_BLOCKS_SLIM"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_092_BONUS_WAVE"), Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_021_CHOICE"),
    Time(1800), IfFloorClear, IfGoalType(b'B'), ThenJumpFloor(1), IfFloorClear, IfGoalType(b'G'),
    ThenJumpFloor(3), Floor("ST_022_BOWL"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_023_JUMPIES"), Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_024_STOPPERS"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_025_FLOOR_BENT"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_026_CONVEYOR"), Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_027_EXAM_B"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_028_CHASER"), IfFloorClear, IfGoalType(b'B'),
    ThenJumpFloor(1), IfFloorClear, IfGoalType(b'G'), ThenJumpFloor(2), IfFloorClear,
    IfGoalType(b'R'), ThenJumpFloor(7), Floor("ST_029_JUMP_DOUBLE"), Time(1800), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_093_BONUS_GRID"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_031_MIDDLE_JAM"), IfFloorClear, ThenJumpFloor(1), Floor("ST_032_ANTLION"), Time(1800),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_033_COLLAPSE"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_034_SWING_BAR"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_035_LABYRINTH"), IfFloorClear, IfGoalType(b'B'), ThenJumpFloor(1), IfFloorClear,
    IfGoalType(b'G'), ThenJumpFloor(3), Floor("ST_036_SPIRAL"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_037_WAVY_JUMP"), Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_038_SPIKY"),
    Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_039_UNREST"), Time(1800), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_040_POLAR"), Time(1800), IfFloorClear, ThenOther, CourseEnd,
];

pub(crate) const EXPERT_MAIN: &[Op] = &[
    Floor("ST_041_RUIN"), Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_042_BRANCH"),
    Time(1800), IfFloorClear, IfGoalType(b'B'), ThenJumpFloor(1), IfFloorClear, IfGoalType(b'R'),
    ThenJumpFloor(3), Floor("ST_043_OVERTURN"), Time(1800), IfFloorClear, IfGoalType(b'B'),
    ThenJumpFloor(1), IfFloorClear, IfGoalType(b'G'), ThenJumpFloor(2), Floor("ST_044_EXCURSION"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_091_BONUS_BASIC"), Time(1800), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_045_DODECAGON"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_046_EXAM_C"), IfFloorClear, ThenJumpFloor(1), Floor("ST_047_SKELETON"), Time(1800),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_048_TRACKS"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_092_BONUS_WAVE"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_051_DOWNHILL_HARD"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_052_GEARS"), IfFloorClear, ThenJumpFloor(1), Floor("ST_053_DESTRUCTION"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_054_INVASION"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_055_DIVING"), Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_056_FLOOR_SLANT"),
    Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_057_TRAM"), Time(1800), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_058_SWING_BAR_LONG"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_059_PAPERWORK"), IfFloorClear, ThenJumpFloor(1), Floor("ST_093_BONUS_GRID"),
    Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_061_TWIN_ATTACKER"), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_062_SEGA_LOGO"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_063_SNAKE"), Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_064_WIND"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_065_WINDY_SLIDE"), Time(1800), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_066_FALL_DOWN"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_067_TWIN_CROSS"), IfFloorClear, ThenJumpFloor(1), Floor("ST_068_SPIRAL_HARD"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_069_CONVEYOR_PARTS"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_094_BONUS_BUMPY"), IfFloorClear, ThenJumpFloor(1), Floor("ST_071_GAPS"), Time(1800),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_072_CURVATURE"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_073_ANT_LION_SUPER"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_074_DRUM"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_075_TWIST_AND_SPIN"), IfFloorClear, ThenJumpFloor(1), Floor("ST_076_SPEEDY_JAM"),
    Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_077_QUAKE"), Time(1800), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_078_CASSIOPEIA"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_079_PIRATES"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_095_BONUS_HUNTING"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_081_BOWL_OPEN"), Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_082_CHECKER"),
    IfFloorClear, IfGoalType(b'B'), ThenJumpFloor(1), IfFloorClear, IfGoalType(b'G'),
    ThenJumpFloor(2), IfFloorClear, IfGoalType(b'R'), ThenJumpFloor(3), Floor("ST_083_CARPET"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_084_RIDGE"), Time(1800), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_085_MIXER"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_086_RINGS"), Time(1800), IfFloorClear, IfGoalType(b'B'), ThenJumpFloor(1),
    IfFloorClear, IfGoalType(b'G'), ThenJumpFloor(2), Floor("ST_087_STAIRS"), Time(1800),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_088_CLOVER"), Time(1800), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_089_COFFEE_CUP"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_090_METAMORPHASIS"), Time(1800), IfFloorClear, ThenOther, CourseEnd,
];

pub(crate) const BEGINNER_EXTRA: &[Op] = &[
    Floor("ST_101_BLUR_BRIDGE"), Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_102_HITTER"),
    Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_103_AV_LOGO"), Time(1800), IfFloorClear,
    ThenOther, CourseEnd,
];

pub(crate) const ADVANCED_EXTRA: &[Op] = &[
    Floor("ST_101_BLUR_BRIDGE"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_104_HARD_HITTER"), Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_105_PUZZLE"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_103_AV_LOGO"), Time(1800), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_106_POLAR_LARGE"), IfFloorClear, ThenOther, CourseEnd,
];

pub(crate) const EXPERT_EXTRA: &[Op] = &[
    Floor("ST_101_BLUR_BRIDGE"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_107_BREATHE"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_104_HARD_HITTER"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_108_FERRIS_WHEEL"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_109_FACTORY"), Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_110_CURL_PIPE"),
    Time(1800), IfFloorClear, ThenJumpFloor(1), Floor("ST_111_MAGIC_HAND"), Time(1800),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_103_AV_LOGO"), Time(1800), IfFloorClear,
    ThenJumpFloor(1), Floor("ST_112_SANCTUARY"), Time(1800), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_113_DAA_LOO_MAA"), Time(1800), IfFloorClear, ThenOther, CourseEnd,
];

pub(crate) const MASTER: &[Op] = &[
    Floor("ST_121_WAVE_MASTER"), IfFloorClear, ThenJumpFloor(1), Floor("ST_122_FAN_MASTER"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_123_STAMINA_MASTER"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_124_SPRING_MASTER"), IfFloorClear, ThenJumpFloor(1), Floor("ST_125_DANCE_MASTER"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_126_ROLL_MASTER"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_127_EDGE_MASTER"), IfFloorClear, ThenJumpFloor(1), Floor("ST_128_DODGE_MASTER"),
    IfFloorClear, ThenJumpFloor(1), Floor("ST_129_BRIDGE_MASTER"), IfFloorClear, ThenJumpFloor(1),
    Floor("ST_130_MONKEY_MASTER"), IfFloorClear, ThenOther, CourseEnd,
];

