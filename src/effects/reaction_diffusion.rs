use super::MAX_TICKS_PER_FRAME;
use crate::{EffectRuntime, hsv_rainbow16, phase_at};

const TICK_MS: u32 = 33;
const STEPS_PER_TICK: u32 = 8;

const ONE: i32 = 1 << 16;
/// Gray-Scott parameters in Q16. At a fixed kill rate, pulses on 60 cells freeze into
/// static bands, so the rate cycles: low values make pulses replicate and high values
/// make them die. Pulses outlive the point where a new seed could start, so a slow
/// rise and quick fall keep the dark gap between generations short.
const FEED: i64 = 2_200;
const MIN_KILL: i64 = 3_800;
const KILL_SPAN: i64 = 520;
const KILL_MS: u32 = 45_000;
/// Share of the cycle spent rising, out of 65536.
const RISE: u32 = 55_700;
const DIFFUSE_U: i64 = 10_486;
const DIFFUSE_V: i64 = 5_243;

/// No `v` above this means nothing is left alive.
const ALIVE: i32 = ONE / 8;
const SEED_RADIUS: usize = 4;
/// A seed dies at once above this kill rate, which would only flicker.
const MAX_SEED_KILL: i64 = MIN_KILL + KILL_SPAN / 4;

pub(super) fn render<const N: usize>(runtime: &mut EffectRuntime<N>) {
    runtime.simulate(TICK_MS, MAX_TICKS_PER_FRAME, tick);
}

fn tick<const N: usize>(runtime: &mut EffectRuntime<N>, time_us: u64) {
    let kill = kill_rate(phase_at(time_us, KILL_MS));

    if kill < MAX_SEED_KILL && runtime.field.iter().all(|cell| cell[1] <= ALIVE) {
        seed(runtime);
    }
    for _ in 0..STEPS_PER_TICK {
        step(runtime, kill);
    }
    for (pixel, cell) in runtime.tick_frame.iter_mut().zip(&runtime.field) {
        // Full colour at v = 0.35, near the peak of a pulse.
        let level = ((cell[1] * 45) >> 12).min(255) as u32;
        let hue = 0x9000 + level * 180;
        let value = 12 + level * 243 / 255;
        *pixel = hsv_rainbow16(hue as u16, 235, value as u8);
    }
}

fn kill_rate(phase: u16) -> i64 {
    let phase = u32::from(phase);
    let height = if phase < RISE {
        phase * 256 / RISE
    } else {
        (65_536 - phase) * 256 / (65_536 - RISE)
    };
    MIN_KILL + ((i64::from(height) * KILL_SPAN) >> 8)
}

fn seed<const N: usize>(runtime: &mut EffectRuntime<N>) {
    let centre = usize::from(runtime.rng.next_u16()) % N;
    let cells = centre.saturating_sub(SEED_RADIUS)..(centre + SEED_RADIUS).min(N);
    for cell in &mut runtime.field[cells] {
        *cell = [ONE / 2, ONE / 2];
    }
}

/// One Gray-Scott step with reflecting ends. `field[i]` is `[1 - u, v]`, so the zeroed
/// runtime state is the correct empty medium. The left neighbour is kept from before
/// its update, so all cells step from the same instant without a second buffer.
fn step<const N: usize>(runtime: &mut EffectRuntime<N>, kill: i64) {
    let mut left = runtime.field[0];
    for index in 0..N {
        let current = runtime.field[index];
        let right = runtime.field[(index + 1).min(N - 1)];
        let (u, v) = (i64::from(ONE - current[0]), i64::from(current[1]));
        let reaction = (((u * v) >> 16) * v) >> 16;
        // The Laplacian of `1 - u` is minus that of `u`.
        let laplacian_u = -i64::from(left[0] + right[0] - 2 * current[0]);
        let laplacian_v = i64::from(left[1] + right[1] - 2 * current[1]);

        let next_u = u + ((laplacian_u * DIFFUSE_U) >> 16) - reaction
            + ((FEED * (i64::from(ONE) - u)) >> 16);
        let next_v = v + ((laplacian_v * DIFFUSE_V) >> 16) + reaction - (((FEED + kill) * v) >> 16);

        runtime.field[index] = [
            ONE - next_u.clamp(0, i64::from(ONE)) as i32,
            next_v.clamp(0, i64::from(ONE)) as i32,
        ];
        left = current;
    }
}
