//! Issue #26 probe: replay a headless frame's instance stream on
//! `rune-ref`, the scalar CPU reference renderer vendored with rune, to
//! see whether it can stand in for Lavapipe as the PNG-golden rasterizer.
//! No GPU, window or display: the input is `Runtime::instances` plus the
//! glyph atlas, exactly what the GPU renderer would have uploaded.

use rune::Runtime;
use rune::text::ATLAS_SIZE;
use rune_ref::{Atlas, Frame, Primitive, Scene};
use std::collections::BTreeMap;

/// What the replay could and couldn't carry over from the frame.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Replay {
    /// Instances per kind (`params[1]`), as rune emitted them.
    pub kinds: BTreeMap<u32, usize>,
    pub glyph_pages: usize,
    /// Glyph instances on an atlas page other than 0 (rune-ref holds one).
    pub off_page_glyphs: usize,
    pub images: usize,
    pub surfaces: usize,
    /// The runtime premultiplies in sRGB encoding (`App::srgb_blend`, on for windows and
    /// `--snap`); rune-ref only models linear blending into an sRGB target.
    pub srgb_blend: bool,
    /// `Scene::validate` verdict (None = valid).
    pub invalid: Option<String>,
}

/// The frame as a `rune-ref` scene, `size` in device pixels.
pub fn scene<S>(rt: &Runtime<S>, size: (u32, u32)) -> (Scene, Replay) {
    let mut rep = Replay {
        glyph_pages: rt.text.atlas.page_count(),
        images: rt.images.pages.len(),
        surfaces: rt.surfaces.len(),
        srgb_blend: rt.srgb_blend(),
        ..Replay::default()
    };
    let primitives = rt
        .instances
        .iter()
        .enumerate()
        .map(|(i, inst)| {
            let kind = inst.params[1] as u32;
            *rep.kinds.entry(kind).or_default() += 1;
            if inst.params[1] == rune::render::KIND_GLYPH && inst.params[2] != 0.0 {
                rep.off_page_glyphs += 1;
            }
            Primitive::from_instance(inst, i as u32 + 1, format!("instance/{i}"))
        })
        .collect();
    let data = rt.text.atlas.page_data(0).unwrap_or_default().to_vec();
    let scene = Scene {
        width: size.0,
        height: size.1,
        clear: [0.0; 4],
        primitives,
        atlas: Atlas {
            width: ATLAS_SIZE,
            height: ATLAS_SIZE,
            data,
        },
    };
    rep.invalid = scene.validate().err();
    (scene, rep)
}

/// Rasterize a valid scene on the CPU oracle (sRGB RGBA8).
pub fn render(scene: &Scene) -> Frame {
    let mut f = Frame::new(scene.width, scene.height, false);
    f.render(scene);
    f
}

/// The frame as PNG bytes (RGBA8, no gamma chunk, fixed compression).
pub fn png(frame: &Frame) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, frame.width, frame.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(&frame.rgba).expect("png data");
    }
    out
}
