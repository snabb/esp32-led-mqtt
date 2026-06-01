use crate::EffectRuntime;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>, now_ms: u32) {
    super::twinkle::render_with_color_mode(runtime, now_ms, true);
}
