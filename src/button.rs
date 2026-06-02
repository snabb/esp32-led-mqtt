use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::Input;
use esp32_led_mqtt::{EFFECT_MAX_CODE, EFFECT_NONE_CODE, random_color};

use crate::light_state;

const DEBOUNCE_MS: u64 = 50;
const DOUBLE_CLICK_MS: u64 = 350;
const LONG_PRESS_MS: u64 = 700;

#[derive(Clone, Copy)]
enum ButtonAction {
    CycleEffect,
    RandomColor,
    TogglePower,
}

enum ButtonPress {
    Short,
    Long,
}

#[embassy_executor::task]
pub async fn task(mut button: Input<'static>) {
    loop {
        wait_debounced_press(&mut button).await;
        match wait_release_or_long_press(&mut button).await {
            ButtonPress::Long => handle_action(ButtonAction::TogglePower, now_ms()),
            ButtonPress::Short => {
                let second_press = select(
                    Timer::after(Duration::from_millis(DOUBLE_CLICK_MS)),
                    async {
                        wait_debounced_press(&mut button).await;
                        wait_release_or_long_press(&mut button).await
                    },
                )
                .await;

                match second_press {
                    Either::First(()) => handle_action(ButtonAction::CycleEffect, now_ms()),
                    Either::Second(ButtonPress::Short) => {
                        handle_action(ButtonAction::RandomColor, now_ms());
                    }
                    Either::Second(ButtonPress::Long) => {
                        handle_action(ButtonAction::TogglePower, now_ms());
                    }
                }
            }
        }
    }
}

async fn wait_debounced_press(button: &mut Input<'_>) {
    loop {
        button.wait_for_falling_edge().await;
        Timer::after(Duration::from_millis(DEBOUNCE_MS)).await;
        if button.is_low() {
            return;
        }
    }
}

async fn wait_debounced_release(button: &mut Input<'_>) {
    loop {
        button.wait_for_rising_edge().await;
        Timer::after(Duration::from_millis(DEBOUNCE_MS)).await;
        if button.is_high() {
            return;
        }
    }
}

async fn wait_release_or_long_press(button: &mut Input<'_>) -> ButtonPress {
    match select(
        Timer::after(Duration::from_millis(LONG_PRESS_MS)),
        wait_debounced_release(button),
    )
    .await
    {
        Either::First(()) => {
            wait_debounced_release(button).await;
            ButtonPress::Long
        }
        Either::Second(()) => ButtonPress::Short,
    }
}

fn handle_action(action: ButtonAction, now_ms: u64) {
    match action {
        ButtonAction::CycleEffect => cycle_effect(),
        ButtonAction::RandomColor => set_random_color(now_ms),
        ButtonAction::TogglePower => {
            light_state::update(|state| state.on = !state.on);
            light_state::mark_dirty();
        }
    }
}

fn cycle_effect() {
    let current = light_state::get().effect_code;
    let next = if current >= EFFECT_MAX_CODE {
        EFFECT_NONE_CODE
    } else {
        current + 1
    };
    light_state::update(|state| state.effect_code = next);
    light_state::mark_dirty();
}

fn set_random_color(now_ms: u64) {
    let previous = light_state::get().color;
    let mut rng = (now_ms as u32)
        ^ (u32::from(previous.r) << 16)
        ^ (u32::from(previous.g) << 8)
        ^ u32::from(previous.b)
        ^ 0x9e37_79b9;
    let color = random_color::from_seed(previous, next_random_u32(&mut rng));

    light_state::update(|state| state.color = color);
    light_state::mark_dirty();
}

fn next_random_u32(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
}
