use core::cell::{Cell, RefCell};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use esp32_led_mqtt::{EffectId, EffectParams, NORMAL_EFFECT_SPEED, light_command::LightState};
use smart_leds::RGB8;

static LIGHT_STATE: critical_section::Mutex<RefCell<LightState>> =
    critical_section::Mutex::new(RefCell::new(DEFAULT_LIGHT_STATE));
/// Set until the state from before the reboot is restored or overridden by a
/// local change; see `restore`.
static RESTORE_PENDING: critical_section::Mutex<Cell<bool>> =
    critical_section::Mutex::new(Cell::new(true));
/// Raised by every `update`; each control plane holds one receiver.
pub(crate) static LIGHT_STATE_CHANGED: Watch<CriticalSectionRawMutex, (), 2> = Watch::new();

const DEFAULT_BRIGHTNESS: u8 = 8;
/// Boots dark until `restore`; the rest is what the button turns on.
const DEFAULT_LIGHT_STATE: LightState = LightState {
    on: false,
    brightness: DEFAULT_BRIGHTNESS,
    effect: Some(EffectId::Rainbow),
    speed: NORMAL_EFFECT_SPEED,
    color: RGB8 {
        r: 255,
        g: 96,
        b: 24,
    },
};

pub(crate) fn get() -> LightState {
    critical_section::with(|cs| *LIGHT_STATE.borrow(cs).borrow())
}

pub(crate) fn update(update: impl FnOnce(&mut LightState)) {
    critical_section::with(|cs| {
        RESTORE_PENDING.borrow(cs).set(false);
        update(&mut LIGHT_STATE.borrow(cs).borrow_mut());
    });
    LIGHT_STATE_CHANGED.sender().send(());
}

#[cfg(feature = "mqtt")]
pub(crate) fn restore_pending() -> bool {
    critical_section::with(|cs| RESTORE_PENDING.borrow(cs).get())
}

/// Applies the state from before the reboot, unless it was already restored
/// or a local change (the button) came first and is therefore newer.
#[cfg(feature = "mqtt")]
pub(crate) fn restore(restore: impl FnOnce(&mut LightState)) {
    let restored = critical_section::with(|cs| {
        RESTORE_PENDING
            .borrow(cs)
            .replace(false)
            .then(|| restore(&mut LIGHT_STATE.borrow(cs).borrow_mut()))
            .is_some()
    });
    if restored {
        LIGHT_STATE_CHANGED.sender().send(());
    }
}

pub(crate) fn effect_params(id: EffectId, state: LightState) -> EffectParams {
    EffectParams {
        id,
        primary: state.color,
        speed: state.speed,
        intensity: 128,
    }
}
