//! The effects compositor: draws one rune-fx layer at each of the shell's
//! effect slots (see [`super`]). A frame without slots is Rune's default
//! frame, untouched, so the compositor changes nothing while effects are
//! off.

use super::{BUILD_GLOW, ONE_SHOT, PUBLISH_FAILED, PUBLISH_OK, RAIL_AURORA, envelope, unpack};
use crate::theme::spring;
use cv_model::workbench::Workbench;
use rune::compose::{Compositor, DeviceKey, Frame};
use rune::render::KIND_EFFECT;
use rune_fx::{
    AuroraParams, Fx, FxFrame, FxLayer, FxOptions, FxRenderer, FxSlot, GlowParams, GlowStyle,
    PulseParams, PulseStyle, Shape, SparkleParams, rgb,
};
use std::collections::HashMap;

struct FxCache {
    device_key: DeviceKey,
    format: wgpu::TextureFormat,
    fx: FxRenderer,
}

/// One-shots are keyed by mark and their slot's sequence number
/// (`effect_data[0]`), and remember the clock time they first showed.
type OneShot = (u32, u32);

#[derive(Default)]
pub struct Gfx {
    fx: Option<FxCache>,
    started: HashMap<OneShot, f64>,
    /// Something drawn last frame still moves: keep the display link up.
    moving: bool,
}

/// The rune-fx layer a slot draws at `strength` (0 draws nothing).
fn layer(mark: u32, d: [f32; 4], strength: f32, shape: Shape, t: f32) -> Option<FxLayer> {
    let fx = match mark {
        RAIL_AURORA => Fx::aurora(AuroraParams {
            intensity: d[0],
            speed: d[1],
            ..Default::default()
        }),
        BUILD_GLOW => Fx::glow(GlowParams {
            style: GlowStyle::Soft,
            color: rgb(unpack(d[1])),
            intensity: d[2] * strength,
            radius: 12.0,
            occlusion: 0.0,
            ..Default::default()
        }),
        PUBLISH_OK => Fx::sparkles(SparkleParams {
            color: rgb(unpack(d[1])),
            intensity: d[2] * strength,
            count: 8,
            ..Default::default()
        }),
        PUBLISH_FAILED => Fx::pulse(PulseParams {
            style: PulseStyle::Radar,
            color: rgb(unpack(d[1])),
            intensity: d[2] * strength,
            period: 0.7,
            ..Default::default()
        }),
        _ => return None,
    };
    (strength > 0.0).then(|| fx.on(shape).at(t))
}

impl Compositor<Workbench> for Gfx {
    fn animating(&self) -> bool {
        self.moving
    }

    fn render(&mut self, f: &mut Frame<'_, Workbench>) {
        let now = f.rt.t;
        let pop = spring(f.rt.state.theme.motion.pop);
        let mut at: Vec<u32> = Vec::new();
        let mut layers: Vec<FxLayer> = Vec::new();
        let mut placements: Vec<FxSlot> = Vec::new();
        let mut seen: Vec<OneShot> = Vec::new();
        let mut moving = false;
        for (n, i) in f.rt.instances.iter().enumerate() {
            if i.params[1] != KIND_EFFECT {
                continue;
            }
            let mark = f.rt.marks.get(n).copied().unwrap_or(0);
            let strength = if mark == RAIL_AURORA {
                moving = true;
                1.0
            } else {
                let id = (mark, i.uv[0].to_bits());
                seen.push(id);
                let age = (now - *self.started.entry(id).or_insert(now)) as f32;
                moving |= age < ONE_SHOT;
                envelope(pop, age)
            };
            let k = i.params[3];
            let [x, y, w, h] = i.rect.map(|v| v / k);
            let r = i.radii.iter().fold(0.0f32, |a, v| a.max(v.abs())) / k;
            let Some(l) = layer(
                mark,
                i.uv,
                strength,
                Shape::rounded(x, y, w, h, r),
                now as f32,
            ) else {
                continue;
            };
            at.push(n as u32);
            layers.push(l);
            placements.push(FxSlot::default().scale(k).clip(i.clip, i.fade));
        }
        self.started.retain(|id, _| seen.contains(id));
        self.moving = moving;
        if layers.is_empty() {
            f.draw_ui();
            return;
        }
        let (device, queue, size) = (f.device, f.queue, f.size);
        let device_key = f.device_key();
        if self
            .fx
            .as_ref()
            .is_none_or(|c| c.device_key != device_key || c.format != f.format)
        {
            self.fx = Some(FxCache {
                device_key,
                format: f.format,
                fx: FxRenderer::new(device, f.format, FxOptions { dither: true }),
            });
        }
        let Some(cache) = self.fx.as_mut() else {
            return;
        };
        let fx = &mut cache.fx;
        fx.set_srgb_blend(f.srgb_blend());
        fx.set_fade_stops(f.fade_stops());
        let frame = FxFrame {
            viewport: [size.0, size.1],
            scale: 1.0,
            layers: &layers,
            backdrop: None,
            sources: &[],
        };
        fx.prepare_with_slots(device, queue, &frame, &placements);
        let clear = f.rt.clear_color();
        f.ui.set_fade_stops(f.rt.fade_stops());
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, size);
        f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
            if let Some(k) = at.iter().position(|n| *n == site.index) {
                fx.draw_layer_in(pass, site.format, site.origin, k);
            }
        });
    }
}
