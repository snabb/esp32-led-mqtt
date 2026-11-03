use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use esp_hal::{gpio::Input, rng::Rng};
use esp32_led_mqtt::{EffectId, random_color};

use crate::light_state;

const DEBOUNCE_MS: u64 = 50;
const DOUBLE_CLICK_MS: u64 = 350;
const LONG_PRESS_MS: u64 = 700;

enum ButtonPress {
    Short,
    Long,
}

#[embassy_executor::task]
pub async fn task(mut button: Input<'static>) {
    loop {
        wait_debounced_press(&mut button).await;
        match wait_release_or_long_press(&mut button).await {
            ButtonPress::Long => toggle_power(),
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
                    Either::First(()) => cycle_effect(),
                    Either::Second(ButtonPress::Short) => set_random_color(),
                    Either::Second(ButtonPress::Long) => toggle_power(),
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

fn toggle_power() {
    light_state::update(|state| state.on = !state.on);
}

fn cycle_effect() {
    light_state::update(|state| state.effect = EffectId::cycle(state.effect));
}

fn set_random_color() {
    let previous = light_state::get().color;
    let color = random_color::from_seed(previous, Rng::new().random());
    light_state::update(|state| state.color = color);
}
