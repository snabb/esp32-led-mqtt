use core::sync::atomic::{AtomicBool, Ordering};

use embassy_net::Stack;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use embassy_time::{Instant, Timer};
use esp_hal_ota::{Ota, OtaImgState};
use esp_storage::FlashStorage;
use esp32_led_mqtt::ota_protocol;
use heapless::String;
use log::{info, warn};

#[cfg(feature = "_control")]
mod download;

#[cfg(feature = "mqtt")]
pub use download::submit;
#[cfg(feature = "_control")]
pub use download::{client, pull_enabled, update};

/// What `flash::task` needs to start OTA downloads, if this build can do them.
#[cfg(feature = "_control")]
pub type DownloadContext = crate::tls::Context;
#[cfg(not(feature = "_control"))]
pub type DownloadContext = ();

pub static LED_READY: AtomicBool = AtomicBool::new(false);
pub static MQTT_READY: AtomicBool = AtomicBool::new(false);
/// Set by the first authenticated CoAP request. Stays set: unlike an MQTT session it has
/// no ongoing state, and the point is only that the image can be reached and controlled.
pub static COAP_AUTHENTICATED: AtomicBool = AtomicBool::new(false);
/// Latest OTA status; each control plane holds one receiver.
pub static STATUS: Watch<CriticalSectionRawMutex, String<192>, 2> = Watch::new();

pub struct FlashAccess<'a>(&'a mut FlashStorage<'static>);
impl embedded_storage::ReadStorage for FlashAccess<'_> {
    type Error = esp_storage::FlashStorageError;
    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.0.read(offset, bytes)
    }
    fn capacity(&self) -> usize {
        self.0.capacity()
    }
}
impl embedded_storage::Storage for FlashAccess<'_> {
    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.write(offset, bytes)
    }
}

fn status(state: &str, detail: &str, percent: u8) {
    crate::memory::report(state);
    let value = heapless::format!(
        "{{\"state\":\"{}\",\"detail\":\"{}\",\"progress\":{},\"running_build\":\"{}\"}}",
        state,
        detail,
        percent,
        env!("FIRMWARE_BUILD_ID")
    )
    .expect("bounded OTA status");
    STATUS.sender().send(value);
}

/// Confirms or rolls back a trial image. `Err` means an unhealthy boot, which must keep
/// flash writes disabled. `control_required` says a control plane is configured, so a
/// trial image must prove it works; a device without one can't be reached to fix it anyway.
pub async fn confirm_boot(
    stack: Option<Stack<'static>>,
    flash: &mut FlashStorage<'static>,
    layout: Result<(), &'static str>,
    control_required: bool,
) -> Result<(), &'static str> {
    let health = match layout {
        Ok(()) => boot_health(stack, flash, control_required).await,
        Err(reason) => Err(reason),
    };
    if let Err(reason) = health {
        warn!("OTA unavailable: {}", reason);
        status("disabled", reason, 0);
    }
    health
}

async fn boot_health(
    stack: Option<Stack<'static>>,
    flash: &mut FlashStorage<'static>,
    control_required: bool,
) -> Result<(), &'static str> {
    let start = Instant::now();
    let mut ota = Ota::new(FlashAccess(flash)).map_err(|_| "partition_table")?;
    let slot = ota.booted_partition().ok_or("unknown_running_slot")?;
    info!(
        "OTA running slot {} build {}",
        slot,
        env!("FIRMWARE_BUILD_ID")
    );
    let state = match ota.image_state() {
        Ok(state) => state,
        Err(_) => match ota.boot_entries() {
            Ok((a, b)) if a.seq == 0 && b.seq == 0 && slot == 0 => {
                ota.set_boot_partition(0, OtaImgState::Valid)
                    .map_err(|_| "initial_metadata")?;
                OtaImgState::Valid
            }
            _ => return Err("unknown_image_state"),
        },
    };
    if state == OtaImgState::PendingVerify {
        status("trial", "health_check", 0);
        loop {
            match ota_protocol::trial_decision(
                (Instant::now() - start).as_millis(),
                LED_READY.load(Ordering::Relaxed),
                !control_required
                    || MQTT_READY.load(Ordering::Acquire)
                    || COAP_AUTHENTICATED.load(Ordering::Acquire),
                stack.is_some_and(|stack| stack.is_config_up()),
            ) {
                ota_protocol::TrialDecision::Confirm => {
                    if ota.mark_app_valid().is_ok() {
                        break;
                    }
                    esp_hal::system::software_reset();
                }
                ota_protocol::TrialDecision::Rollback => {
                    warn!("OTA trial failed health check; rebooting for rollback");
                    esp_hal::system::software_reset();
                }
                ota_protocol::TrialDecision::Wait => {}
            }
            Timer::after_millis(100).await;
        }
    } else if !matches!(state, OtaImgState::Valid | OtaImgState::Undefined) {
        // NEW should have become PENDING_VERIFY in a rollback-enabled bootloader.
        return Err("bootloader_state");
    }
    Ok(())
}
