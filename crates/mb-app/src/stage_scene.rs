//! Assembling a stage (stagedef + stage GMA/TPL + common + background) into renderer objects.
//! Mirrors `loadRenderStage` in the reference `main.ts` and the model lookup order of `ModelCache`
//! (stage GMA first, then the background GMA, then the stage NaomiLib archive).

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::rc::Rc;

use glam::{Mat4, Vec3};
use mb_render::{Layer, ModelId, ObjectId, Renderer, TextureId, TextureSet};
use mb_stage::{S16Vec, Stage};
use web_time::Instant;

use crate::anim::world_from_group;
use crate::content::{
    common_files, stage_bg, Content, StageFiles, Wanted, COMMON_GMA, COMMON_NL_OBJ, COMMON_NL_TPL,
    COMMON_TPL,
};
use crate::stage_info::BgInfo;

pub const S16_TO_RADIANS: f32 = std::f32::consts::PI / 32768.0;

/// Stages whose geometry lives in a NaomiLib archive (`stNNN_p.lz`) instead of a GMA.
pub const NAOMI_STAGE_IDS: [u32; 22] = [
    10, 19, 20, 30, 49, 50, 60, 70, 80, 92, 96, 97, 98, 99, 100, 114, 115, 116, 117, 118, 119, 120,
];

pub fn is_naomi_stage(id: u32) -> bool {
    NAOMI_STAGE_IDS.contains(&id)
}

#[derive(Debug)]
pub enum LoadError {
    Io(String, io::Error),
    Parse(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(what, e) => write!(f, "{what}: {e}"),
            LoadError::Parse(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// NaomiLib stage geometry plus its texture container.
pub struct NaomiStage {
    pub obj: mb_gma::NlObj,
    pub tpl: mb_tpl::Tpl,
}

/// The files every stage shares, parsed once: ball, bananas and goal bag (`common.gma` / `.tpl`), goal
/// tape and clock digits (the common NaomiLib archive; missing files only cost that decoration).
pub struct CommonAssets {
    pub gma: mb_gma::Gma,
    pub tpl: mb_tpl::Tpl,
    pub nl: Option<(mb_gma::NlObj, mb_tpl::Tpl)>,
}

/// One parsed background.
pub struct BgAssets {
    pub file_name: &'static str,
    pub gma: mb_gma::Gma,
    pub tpl: mb_tpl::Tpl,
}

/// Parsed files kept between stage loads so they are not read and parsed again: the shared files (always)
/// and the background of the last stage (consecutive floors of a course nearly always share it).
///
/// Budget: what is held is the parsed form of four small shared files (about 1.2 MB on the USA disc) and one
/// background (0.5 to 3.5 MB); nothing else accumulates. Uploaded textures and meshes are not kept here:
/// a stage uploads only the textures its models use (see [`LazyTextures`]).
#[derive(Default)]
pub struct AssetCache {
    common: Option<Rc<CommonAssets>>,
    bg: Option<Rc<BgAssets>>,
}

impl AssetCache {
    /// What a load of `files` will read, given what is cached (music excluded: the audio system knows
    /// whether it needs the streams).
    pub fn wanted(&self, files: &StageFiles) -> Vec<Wanted> {
        let mut v = files.stage_wanted();
        if !self.has_bg(files.bg.file_name) {
            v.extend(files.bg_wanted());
        }
        if self.common.is_none() {
            v.extend(common_files());
        }
        v
    }

    fn has_bg(&self, file_name: &str) -> bool {
        self.bg.as_ref().is_some_and(|b| b.file_name == file_name)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Every parsed file a stage needs. Pure CPU data, no GPU.
pub struct StageAssets {
    pub id: u32,
    pub bg: BgInfo,
    pub stagedef: Stage,
    /// Absent for pure-NaomiLib stages.
    pub stage_gma: Option<(mb_gma::Gma, mb_tpl::Tpl)>,
    pub naomi: Option<NaomiStage>,
    pub common: Rc<CommonAssets>,
    pub backdrop: Rc<BgAssets>,
}

fn io_err(what: &str, e: io::Error) -> LoadError {
    LoadError::Io(what.to_string(), e)
}

/// Where the time of one stage load went, in milliseconds (shown by the web shell's `mbPerf.lastLoad()`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoadTimings {
    pub stage_id: u32,
    /// Reading files from the disc image (includes RVZ group decompression).
    pub disc_read_ms: f64,
    pub disc_bytes: usize,
    /// LZSS decompression of `.lz` files.
    pub lzss_ms: f64,
    pub lzss_bytes: usize,
    /// Parsing stagedef, GMA, NaomiLib and TPL headers.
    pub parse_ms: f64,
    /// Decoding GX texture formats to RGBA.
    pub texture_decode_ms: f64,
    /// Creating GPU textures and handing over the texels.
    pub texture_upload_ms: f64,
    pub texture_bytes: usize,
    pub textures: usize,
    /// Display lists to vertices, vertex/index buffers, materials and pipelines.
    pub mesh_ms: f64,
    /// Everything after the files were parsed (the three lines above plus object setup).
    pub gpu_total_ms: f64,
    pub total_ms: f64,
}

impl LoadTimings {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"stage\":{},\"totalMs\":{:.1},\"discReadMs\":{:.1},\"discBytes\":{},\"lzssMs\":{:.1},\
             \"lzssBytes\":{},\"parseMs\":{:.1},\"textureDecodeMs\":{:.1},\"textureUploadMs\":{:.1},\
             \"textureBytes\":{},\"textures\":{},\"meshMs\":{:.1},\"gpuTotalMs\":{:.1}}}",
            self.stage_id,
            self.total_ms,
            self.disc_read_ms,
            self.disc_bytes,
            self.lzss_ms,
            self.lzss_bytes,
            self.parse_ms,
            self.texture_decode_ms,
            self.texture_upload_ms,
            self.texture_bytes,
            self.textures,
            self.mesh_ms,
            self.gpu_total_ms,
        )
    }
}

impl StageAssets {
    /// `bg` overrides the background normally chosen from the stage id.
    pub fn load(content: &Content, stage_id: u32, bg: Option<BgInfo>) -> Result<Self, LoadError> {
        let name = mb_game::stages::bg_name(stage_id).unwrap_or_default();
        let files = StageFiles::new(stage_id, bg.unwrap_or(stage_bg(stage_id, name)), name);
        Self::load_timed(
            content,
            &files,
            &mut AssetCache::default(),
            &mut LoadTimings::default(),
        )
    }

    /// Read and parse the files of a stage, taking the shared files and the background from `cache`
    /// when they are there (and leaving them there for the next stage). Records where the time went.
    pub fn load_timed(
        content: &Content,
        files: &StageFiles,
        cache: &mut AssetCache,
        timings: &mut LoadTimings,
    ) -> Result<Self, LoadError> {
        let started = Instant::now();
        // (disc ms, disc bytes, lzss ms, lzss bytes)
        let io_time = Cell::new((0.0f64, 0usize, 0.0f64, 0usize));
        let ms = |t: Instant| (Instant::now() - t).as_secs_f64() * 1000.0;
        let read = |path: &str| -> io::Result<Vec<u8>> {
            let t = Instant::now();
            let bytes = content.read(path)?;
            let (d, db, l, lb) = io_time.get();
            io_time.set((d + ms(t), db + bytes.len(), l, lb));
            Ok(bytes)
        };
        let unpack = |path: &str, raw: Vec<u8>| -> io::Result<Vec<u8>> {
            let t = Instant::now();
            let out = mb_assets::lzss::decompress(&raw)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{path}: {e}")))?;
            let (d, db, l, lb) = io_time.get();
            io_time.set((d, db, l + ms(t), lb + out.len()));
            Ok(out)
        };
        let read_lz = |path: &str| -> io::Result<Vec<u8>> { unpack(path, read(path)?) };
        // Like `Content::read_maybe_lz`: the plain file, else `path.lz`.
        let read_maybe_lz = |path: &str| -> io::Result<Vec<u8>> {
            match read(path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => read_lz(&format!("{path}.lz")),
                other => other,
            }
        };
        let result = Self::load_with(files, cache, &read_lz, &read_maybe_lz);
        let (disc_ms, disc_bytes, lzss_ms, lzss_bytes) = io_time.get();
        timings.stage_id = files.stage_id;
        timings.disc_read_ms = disc_ms;
        timings.disc_bytes = disc_bytes;
        timings.lzss_ms = lzss_ms;
        timings.lzss_bytes = lzss_bytes;
        timings.parse_ms = (ms(started) - disc_ms - lzss_ms).max(0.0);
        result
    }

    fn load_with(
        files: &StageFiles,
        cache: &mut AssetCache,
        read_lz: &dyn Fn(&str) -> io::Result<Vec<u8>>,
        read_maybe_lz: &dyn Fn(&str) -> io::Result<Vec<u8>>,
    ) -> Result<Self, LoadError> {
        let get = |path: &str| -> Result<Vec<u8>, LoadError> {
            read_maybe_lz(path).map_err(|e| io_err(path, e))
        };
        let get_lz = |path: &str| -> Result<Vec<u8>, LoadError> {
            read_lz(path).map_err(|e| io_err(path, e))
        };
        let stagedef_bytes = get_lz(&files.stagedef)?;
        let stagedef = mb_stage::parse_stagedef_smb1(&stagedef_bytes)
            .map_err(|e| LoadError::Parse(format!("{}: {e:?}", files.stagedef)))?;

        let parse_gma = |path: &str| -> Result<mb_gma::Gma, LoadError> {
            mb_gma::parse_gma(&get(path)?).map_err(|e| LoadError::Parse(format!("{path}: {e}")))
        };
        let parse_tpl = |bytes: Vec<u8>, path: &str| -> Result<mb_tpl::Tpl, LoadError> {
            mb_tpl::parse(&bytes).map_err(|e| LoadError::Parse(format!("{path}: {e}")))
        };
        let tpl = |path: &str| -> Result<mb_tpl::Tpl, LoadError> { parse_tpl(get(path)?, path) };

        // Parsed straight from the probe read (the file used to be read and decompressed twice).
        let stage_gma = match read_maybe_lz(&files.gma) {
            Ok(bytes) => Some((
                mb_gma::parse_gma(&bytes)
                    .map_err(|e| LoadError::Parse(format!("{}: {e}", files.gma)))?,
                tpl(&files.tpl)?,
            )),
            Err(e) if e.kind() == io::ErrorKind::NotFound && files.naomi => None,
            Err(e) => return Err(io_err(&files.gma, e)),
        };
        let naomi = if files.naomi {
            let obj = mb_gma::parse_naomi_obj(&get_lz(&files.naomi_obj)?)
                .map_err(|e| LoadError::Parse(format!("{}: {e}", files.naomi_obj)))?;
            Some(NaomiStage {
                obj,
                tpl: parse_tpl(get_lz(&files.naomi_tpl)?, &files.naomi_tpl)?,
            })
        } else {
            None
        };
        let common = match &cache.common {
            Some(c) => c.clone(),
            None => {
                // The goal tape and clock digits are decoration: a disc without them still plays.
                let nl = || -> Option<(mb_gma::NlObj, mb_tpl::Tpl)> {
                    let obj = read_lz(COMMON_NL_OBJ).ok()?;
                    let tpl = read_lz(COMMON_NL_TPL).ok()?;
                    Some((
                        mb_gma::parse_naomi_obj(&obj).ok()?,
                        mb_tpl::parse(&tpl).ok()?,
                    ))
                };
                let c = Rc::new(CommonAssets {
                    gma: parse_gma(COMMON_GMA)?,
                    tpl: tpl(COMMON_TPL)?,
                    nl: nl(),
                });
                cache.common = Some(c.clone());
                c
            }
        };
        let backdrop = match &cache.bg {
            Some(b) if b.file_name == files.bg.file_name => b.clone(),
            _ => {
                let b = Rc::new(BgAssets {
                    file_name: files.bg.file_name,
                    gma: parse_gma(&files.bg_gma)?,
                    tpl: tpl(&files.bg_tpl)?,
                });
                cache.bg = Some(b.clone());
                b
            }
        };
        Ok(Self {
            id: files.stage_id,
            bg: files.bg,
            stagedef,
            stage_gma,
            naomi,
            common,
            backdrop,
        })
    }
}

/// `T * Rz * Ry * Rx * S` with S16 angles, the composition order the reference uses everywhere.
pub fn trs_s16(pos: Vec3, rot: S16Vec, scale: Vec3) -> Mat4 {
    Mat4::from_translation(pos)
        * Mat4::from_rotation_z(rot.z as f32 * S16_TO_RADIANS)
        * Mat4::from_rotation_y(rot.y as f32 * S16_TO_RADIANS)
        * Mat4::from_rotation_x(rot.x as f32 * S16_TO_RADIANS)
        * Mat4::from_scale(scale)
}

fn v3(v: mb_stage::Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

/// Renderer objects created for a stage.
pub struct StageScene {
    /// Everything parented to an anim group (its models, goals, bumpers, bananas, jamabars).
    pub items: Vec<Item>,
    pub bg_objects: Vec<ObjectId>,
    pub fg_objects: Vec<ObjectId>,
    /// For each entry of `bg_objects` / `fg_objects`: its index in the stagedef list.
    pub bg_sources: Vec<usize>,
    pub fg_sources: Vec<usize>,
    /// The `common.tpl` textures uploaded so far, for states that add more `common.gma` models (ball,
    /// goal bag).
    pub common_set: LazyTextures,
    pub start: Vec3,
    /// Model names that could not be found in any source.
    pub missing_models: Vec<String>,
    /// Texture count before this stage was uploaded, for `Renderer::truncate_textures`.
    pub texture_base: usize,
}

/// How an item moves relative to its anim group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    Static,
    /// Spins about its own Y axis (bumpers), S16 units per 60 Hz frame.
    LocalSpinY(f32),
    /// Keeps world orientation and spins about world Y at the group-transformed position (bananas).
    WorldSpinY(f32),
}

/// What a stage item is, with its index in the simulation's runtime lists (`mb_engine::StageRuntime`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Model,
    /// Index over all goals in anim-group order.
    Goal(usize),
    /// Index within the item's anim group.
    Bumper(usize),
    Jamabar(usize),
    /// Index over all bananas in anim-group order.
    Banana(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub object: ObjectId,
    pub group: usize,
    pub local: Mat4,
    pub motion: Motion,
    pub kind: ItemKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Src {
    Stage,
    Bg,
    Common,
}

/// The textures of one TPL, uploaded on first use: a stage decodes and uploads only what its models
/// reference (the background and shared TPLs hold many textures no single stage shows).
#[derive(Default)]
pub struct LazyTextures {
    set: TextureSet,
    /// Indices already tried (uploaded, or undecodable and left to the white fallback).
    tried: Vec<bool>,
}

impl LazyTextures {
    /// Make sure texture `index` of `tpl` is on the GPU.
    pub fn ensure(&mut self, r: &mut Renderer, tpl: &mb_tpl::Tpl, index: usize) {
        if self.tried.get(index).copied().unwrap_or(false) {
            return;
        }
        if self.tried.len() <= index {
            self.tried.resize(index + 1, false);
            self.set.resize(index + 1, None);
        }
        self.tried[index] = true;
        self.set[index] = tpl.get(index).and_then(|tex| r.upload_tpl_texture(tex));
    }

    /// Upload what `model` samples and return the set to hand to `Renderer::upload_model`.
    pub fn for_model(
        &mut self,
        r: &mut Renderer,
        tpl: &mb_tpl::Tpl,
        model: &mb_gma::Model,
    ) -> &[Option<TextureId>] {
        for layer in &model.tev_layers {
            if let Ok(index) = usize::try_from(layer.tex_idx) {
                self.ensure(r, tpl, index);
            }
        }
        &self.set
    }

    pub fn for_nl_model(
        &mut self,
        r: &mut Renderer,
        tpl: &mb_tpl::Tpl,
        model: &mb_gma::NlModel,
    ) -> &[Option<TextureId>] {
        for mesh in &model.meshes {
            if let Some(index) = mesh.tex_idx {
                self.ensure(r, tpl, index as usize);
            }
        }
        &self.set
    }
}

struct ModelLoader<'a> {
    assets: &'a StageAssets,
    stage_set: LazyTextures,
    bg_set: LazyTextures,
    nl_set: LazyTextures,
    common_set: LazyTextures,
    cache: HashMap<(Src, String), Option<ModelId>>,
    nl_cache: HashMap<String, Option<ModelId>>,
}

impl ModelLoader<'_> {
    fn get(&mut self, r: &mut Renderer, src: Src, name: &str) -> Option<ModelId> {
        if let Some(hit) = self.cache.get(&(src, name.to_string())) {
            return *hit;
        }
        let a = self.assets;
        let source = match src {
            Src::Stage => a
                .stage_gma
                .as_ref()
                .map(|(gma, tpl)| (gma, tpl, &mut self.stage_set)),
            Src::Bg => Some((&a.backdrop.gma, &a.backdrop.tpl, &mut self.bg_set)),
            Src::Common => Some((&a.common.gma, &a.common.tpl, &mut self.common_set)),
        };
        let id = source.and_then(|(gma, tpl, set)| {
            let model = gma.model(name)?;
            let textures = set.for_model(r, tpl, model).to_vec();
            r.upload_model(model, &textures).ok()
        });
        self.cache.insert((src, name.to_string()), id);
        id
    }

    /// NaomiLib lookup like the reference's `resolveStageModel`: exact name, `NAME_MAP`, then the longest
    /// name that starts with `name`.
    fn get_naomi(&mut self, r: &mut Renderer, name: &str) -> Option<ModelId> {
        if let Some(hit) = self.nl_cache.get(name) {
            return *hit;
        }
        let id = self.assets.naomi.as_ref().and_then(|n| {
            let names = &n.obj.names;
            let idx = names
                .get(name)
                .or_else(|| names.get(&format!("{name}_MAP")))
                .or_else(|| {
                    names
                        .iter()
                        .filter(|(k, _)| k.starts_with(name))
                        .max_by_key(|(k, _)| k.len())
                        .map(|(_, v)| v)
                })
                .copied()?;
            let model = n.obj.model(idx)?;
            let textures = self.nl_set.for_nl_model(r, &n.tpl, model).to_vec();
            Some(r.upload_nl_model(model, &textures))
        });
        self.nl_cache.insert(name.to_string(), id);
        id
    }

    /// Background-specific model (`JUN_GOAL`, `SUN_BUMPER_L1`, ...): first model whose name minus its 4 character
    /// prefix equals `postfix`, searching stage, background, then common (`findBgSpecificModel`).
    fn find_bg_specific(&mut self, r: &mut Renderer, postfix: &str) -> Option<ModelId> {
        for src in [Src::Stage, Src::Bg, Src::Common] {
            let gma = match src {
                Src::Stage => self.assets.stage_gma.as_ref().map(|s| &s.0),
                Src::Bg => Some(&self.assets.backdrop.gma),
                Src::Common => Some(&self.assets.common.gma),
            };
            let name = gma.and_then(|g| {
                g.names()
                    .find(|n| n.get(4..) == Some(postfix))
                    .map(str::to_string)
            });
            if let Some(name) = name {
                return self.get(r, src, &name);
            }
        }
        None
    }

    fn find_substring(&mut self, r: &mut Renderer, needle: &str) -> Option<ModelId> {
        for src in [Src::Stage, Src::Bg, Src::Common] {
            let gma = match src {
                Src::Stage => self.assets.stage_gma.as_ref().map(|s| &s.0),
                Src::Bg => Some(&self.assets.backdrop.gma),
                Src::Common => Some(&self.assets.common.gma),
            };
            let name = gma.and_then(|g| {
                g.names()
                    .find(|n| n.to_ascii_uppercase().contains(needle))
                    .map(str::to_string)
            });
            if let Some(name) = name {
                return self.get(r, src, &name);
            }
        }
        None
    }

    /// Stage GMA, then background GMA, then the stage NaomiLib archive.
    fn resolve(&mut self, r: &mut Renderer, name: &str) -> Option<ModelId> {
        self.get(r, Src::Stage, name)
            .or_else(|| self.get(r, Src::Bg, name))
            .or_else(|| self.get_naomi(r, name))
    }
}

fn place_bg_objects(
    r: &mut Renderer,
    loader: &mut ModelLoader,
    list: &[mb_stage::BgObject],
    layer: Layer,
    missing: &mut Vec<String>,
) -> (Vec<ObjectId>, Vec<usize>) {
    let mut out = Vec::new();
    let mut sources = Vec::new();
    for (index, o) in list.iter().enumerate() {
        if o.flags & 1 == 0 {
            continue;
        }
        match loader.resolve(r, &o.name) {
            Some(id) => {
                let obj = r.add_object(id, layer, trs_s16(v3(o.pos), o.rot, v3(o.scale)));
                r.set_alpha(obj, 1.0 - o.translucency);
                out.push(obj);
                sources.push(index);
            }
            None => missing.push(o.name.clone()),
        }
    }
    (out, sources)
}

impl StageScene {
    pub fn build(r: &mut Renderer, a: &StageAssets) -> Self {
        let texture_base = r.texture_count();
        r.lighting = a.bg.lighting();
        r.clear_color = a.bg.clear_color.map(|c| c as f64);
        let mut loader = ModelLoader {
            assets: a,
            stage_set: LazyTextures::default(),
            bg_set: LazyTextures::default(),
            common_set: LazyTextures::default(),
            nl_set: LazyTextures::default(),
            cache: HashMap::new(),
            nl_cache: HashMap::new(),
        };
        let mut missing = Vec::new();

        let mut items = Vec::new();
        let goal_blue = loader
            .find_bg_specific(r, "GOAL")
            .or_else(|| loader.find_bg_specific(r, "GOAL_B"));
        let goal_green = loader.find_bg_specific(r, "GOAL_G");
        let goal_red = loader.find_bg_specific(r, "GOAL_R");
        let bumper = loader.find_bg_specific(r, "BUMPER_L1");
        let jamabar = loader.find_substring(r, "JAMABAR");
        let banana_single = loader.get(r, Src::Common, "OBJ_BANANA_01_LOD150");
        let banana_bunch = loader.get(r, Src::Common, "OBJ_BANANA_02_LOD100");
        let mut add = |r: &mut Renderer, model: Option<ModelId>, group, local, motion, kind| {
            if let Some(m) = model {
                let object = r.add_object(m, Layer::World, Mat4::IDENTITY);
                items.push(Item {
                    object,
                    group,
                    local,
                    motion,
                    kind,
                });
            }
        };
        let (mut goal_index, mut banana_index) = (0, 0);
        for (gi, g) in a.stagedef.anim_groups.iter().enumerate() {
            for m in &g.models {
                match loader.resolve(r, &m.name) {
                    Some(id) => add(
                        r,
                        Some(id),
                        gi,
                        Mat4::IDENTITY,
                        Motion::Static,
                        ItemKind::Model,
                    ),
                    None if m.name.starts_with("null") => {}
                    None => missing.push(m.name.clone()),
                }
            }
            for goal in &g.goals {
                let model = match goal.kind {
                    mb_stage::GoalType::Blue => goal_blue,
                    mb_stage::GoalType::Green => goal_green,
                    mb_stage::GoalType::Red => goal_red,
                };
                let local = trs_s16(v3(goal.pos), goal.rot, Vec3::ONE);
                let kind = ItemKind::Goal(goal_index);
                add(r, model, gi, local, Motion::Static, kind);
                goal_index += 1;
            }
            for (i, b) in g.bumpers.iter().enumerate() {
                let local = trs_s16(v3(b.pos), b.rot, v3(b.scale));
                let motion = Motion::LocalSpinY(0x100 as f32);
                add(r, bumper, gi, local, motion, ItemKind::Bumper(i));
            }
            for (i, j) in g.jamabars.iter().enumerate() {
                let local = trs_s16(v3(j.pos), j.rot, v3(j.scale));
                add(r, jamabar, gi, local, Motion::Static, ItemKind::Jamabar(i));
            }
            for b in &g.bananas {
                let (model, rate) = if b.kind == 0 {
                    (banana_single, 1024.0)
                } else {
                    (banana_bunch, 768.0)
                };
                let local = Mat4::from_translation(v3(b.pos));
                let kind = ItemKind::Banana(banana_index);
                add(r, model, gi, local, Motion::WorldSpinY(rate), kind);
                banana_index += 1;
            }
        }

        let (bg_objects, bg_sources) = place_bg_objects(
            r,
            &mut loader,
            &a.stagedef.bg_objects,
            Layer::Background,
            &mut missing,
        );
        let (fg_objects, fg_sources) = place_bg_objects(
            r,
            &mut loader,
            &a.stagedef.fg_objects,
            Layer::World,
            &mut missing,
        );

        let start = a
            .stagedef
            .start_positions
            .first()
            .map(|s| v3(s.pos))
            .unwrap_or(Vec3::ZERO);
        let scene = Self {
            items,
            bg_objects,
            fg_objects,
            bg_sources,
            fg_sources,
            common_set: loader.common_set,
            start,
            missing_models: missing,
            texture_base,
        };
        scene.set_time(r, &a.stagedef, 0.0);
        scene
    }

    /// Pose every item at `time` seconds via `Renderer::set_transform`: anim groups follow their keyframes,
    /// bumpers and bananas spin at the reference's per-frame rates.
    pub fn set_time(&self, r: &mut Renderer, stage: &Stage, time: f32) {
        let range = (stage.loop_start_seconds, stage.loop_end_seconds);
        let groups: Vec<Mat4> = stage
            .anim_groups
            .iter()
            .enumerate()
            .map(|(i, g)| world_from_group(g, i, time, range))
            .collect();
        let frames = time * 60.0;
        for it in &self.items {
            let world = groups.get(it.group).copied().unwrap_or(Mat4::IDENTITY);
            let m = match it.motion {
                Motion::Static => world * it.local,
                Motion::LocalSpinY(rate) => {
                    world * it.local * Mat4::from_rotation_y(spin_angle(rate, frames))
                }
                Motion::WorldSpinY(rate) => {
                    let pos = world.transform_point3(it.local.w_axis.truncate());
                    Mat4::from_translation(pos) * Mat4::from_rotation_y(spin_angle(rate, frames))
                }
            };
            r.set_transform(it.object, m);
        }
    }
}

/// Spin angle in radians after `frames` 60 Hz frames at `rate` S16 units per frame (wrapped like the reference).
fn spin_angle(rate: f32, frames: f32) -> f32 {
    (rate * S16_TO_RADIANS * frames) % (2.0 * std::f32::consts::PI)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trs_applies_translation_after_rotation() {
        let m = trs_s16(
            Vec3::new(1.0, 2.0, 3.0),
            S16Vec {
                x: 0,
                y: 0x4000,
                z: 0,
            },
            Vec3::ONE,
        );
        // +X rotated a quarter turn about Y points down -Z.
        let p = m.transform_point3(Vec3::X);
        assert!((p - Vec3::new(1.0, 2.0, 2.0)).length() < 1e-5, "{p:?}");
    }

    #[test]
    fn scale_applies_before_rotation() {
        let m = trs_s16(
            Vec3::ZERO,
            S16Vec {
                x: 0,
                y: 0x4000,
                z: 0,
            },
            Vec3::new(2.0, 1.0, 1.0),
        );
        let p = m.transform_point3(Vec3::X);
        assert!((p - Vec3::new(0.0, 0.0, -2.0)).length() < 1e-5, "{p:?}");
    }

    #[test]
    fn naomi_stage_list_matches_reference() {
        assert!(is_naomi_stage(10) && is_naomi_stage(120));
        assert!(!is_naomi_stage(1) && !is_naomi_stage(2));
    }
}
