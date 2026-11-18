use crate::{EffectRuntime, blend_rgb, scale8};
use smart_leds::RGB8;

const ONE: u32 = 65_536;

const fn rgb(r: u8, g: u8, b: u8) -> RGB8 {
    RGB8 { r, g, b }
}

fn pixel_center<const N: usize>(index: usize) -> u32 {
    ((index as u64 * u64::from(ONE) + u64::from(ONE / 2)) / N as u64) as u32
}

fn eased(weight: u8) -> u8 {
    let x = u32::from(weight);
    ((x * x * (3 * 255 - 2 * x) + 255 * 255 / 2) / (255 * 255)) as u8
}

const COOL: [RGB8; 5] = [
    rgb(68, 128, 228),
    rgb(34, 194, 159),
    rgb(143, 81, 218),
    rgb(63, 164, 204),
    rgb(86, 93, 201),
];
const JEWEL: [RGB8; 5] = [
    rgb(53, 158, 181),
    rgb(203, 156, 55),
    rgb(159, 71, 197),
    rgb(79, 112, 211),
    rgb(212, 89, 119),
];
const WARM: [RGB8; 5] = [
    rgb(220, 101, 86),
    rgb(222, 172, 65),
    rgb(162, 67, 155),
    rgb(215, 131, 79),
    rgb(203, 75, 117),
];
const PANE_ORDERS: [[usize; 5]; 6] = [
    [0, 1, 2, 3, 4],
    [1, 0, 2, 3, 4],
    [1, 0, 3, 2, 4],
    [1, 4, 3, 2, 0],
    [4, 1, 3, 2, 0],
    [4, 1, 2, 3, 0],
];
const EXCHANGE_MS: u32 = 6_000;

pub(super) fn stained_glass<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let variation = runtime.params.variation;
    let (first, second, mix) = if variation <= 128 {
        (COOL, JEWEL, (u16::from(variation) * 255 / 128) as u8)
    } else {
        (JEWEL, WARM, (u16::from(variation - 128) * 255 / 127) as u8)
    };
    let palette: [RGB8; 5] = core::array::from_fn(|i| blend_rgb(first[i], second[i], mix));
    let step = runtime.step(EXCHANGE_MS) as usize % PANE_ORDERS.len();
    let next = (step + 1) % PANE_ORDERS.len();
    let weight = eased((runtime.phase(EXCHANGE_MS) >> 8) as u8);
    let panes: [RGB8; 5] = core::array::from_fn(|i| {
        blend_rgb(
            palette[PANE_ORDERS[step][i]],
            palette[PANE_ORDERS[next][i]],
            weight,
        )
    });

    // Each blend is centered on a pane boundary. The outer panes extend to the ends.
    const HALF_SEAM: u32 = ONE / 12;
    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let position = pixel_center::<N>(index) * 5 + HALF_SEAM;
        let right = (position / ONE) as usize;
        let left = right.saturating_sub(1).min(4);
        let weight = ((position % ONE).min(2 * HALF_SEAM) * 255 / (2 * HALF_SEAM)) as u8;
        *pixel = blend_rgb(panes[left], panes[right.min(4)], eased(weight));
    }
}

const BLOOM_US: u64 = 24_000_000;
const BLOOMS: [(u32, RGB8, RGB8); 3] = [
    (ONE / 2, rgb(217, 121, 76), rgb(205, 71, 126)),
    (ONE * 2 / 5, rgb(113, 212, 151), rgb(49, 151, 201)),
    (ONE * 3 / 5, rgb(174, 115, 224), rgb(65, 104, 218)),
];
const INDIGO: RGB8 = rgb(34, 30, 87);

pub(super) fn color_bloom<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let cycle = (runtime.clock_us / BLOOM_US % BLOOMS.len() as u64) as usize;
    let age = runtime.clock_us % BLOOM_US;
    let (center, inner, outer) = BLOOMS[cycle];
    let opened = eased((age.min(8_000_000) * 255 / 8_000_000) as u8);
    let opacity = opened.min(eased(
        ((BLOOM_US - age).min(11_000_000) * 255 / 11_000_000) as u8,
    ));
    let full_radius = ONE / 8 + ONE / 4 * u32::from(runtime.params.variation) / 255;
    let start_radius = ONE / 24;
    let radius = start_radius + (full_radius - start_radius) * u32::from(opened) / 255;

    for (index, pixel) in runtime.frame.iter_mut().enumerate() {
        let distance = pixel_center::<N>(index).abs_diff(center);
        let radial = (distance.min(radius) * 255 / radius) as u8;
        let feather = eased(radial.saturating_sub(127).saturating_mul(2));
        let shade = blend_rgb(inner, outer, radial.saturating_mul(2));
        *pixel = blend_rgb(INDIGO, shade, scale8(255 - feather, opacity));
    }
}
