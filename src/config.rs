use core::sync::atomic::{AtomicU32, Ordering};
use embassy_embedded_hal::adapter::{BlockingAsync, YieldingAsync};
use embassy_time::Timer;
use embedded_io_async::Read;
use esp_hal::{Async, usb::usb_serial_jtag::UsbSerialJtagRx};
use esp_storage::FlashStorage;
use esp32_led_mqtt::{
    config_store,
    device_config::{self, DeviceConfig},
    provisioning::{self, Command},
};

use crate::flash::{self, Job};

static GENERATION: AtomicU32 = AtomicU32::new(0);
/// The device's public CCS credential in hex, empty until it has a device key.
#[cfg(feature = "coap")]
static DEVICE_CREDENTIAL: critical_section::Mutex<
    core::cell::RefCell<device_config::CoapCredential>,
> = critical_section::Mutex::new(core::cell::RefCell::new(heapless::String::new()));

/// Remembers the public credential, for `status` replies.
#[cfg(feature = "coap")]
fn set_device_credential(config: &DeviceConfig) {
    let credential = esp32_led_mqtt::coap_keys::public_credential(&config.coap_device_key);
    critical_section::with(|cs| {
        *DEVICE_CREDENTIAL.borrow_ref_mut(cs) = credential.unwrap_or_default();
    });
}

/// `,"device":…,"coap_ccs":…` once the device has a key, or nothing.
#[cfg(feature = "coap")]
fn device_credential_fields() -> heapless::String<320> {
    let credential = critical_section::with(|cs| DEVICE_CREDENTIAL.borrow_ref(cs).clone());
    if credential.is_empty() {
        return heapless::String::new();
    }
    let mac = esp_hal::efuse::base_mac_address();
    let mac = mac.as_bytes();
    let device = esp32_led_mqtt::device_slug([mac[3], mac[4], mac[5]]);
    heapless::format!(",\"device\":\"{device}\",\"coap_ccs\":\"{credential}\"")
        .expect("bounded credential fields")
}

#[cfg(not(feature = "coap"))]
fn device_credential_fields() -> &'static str {
    ""
}

/// Requires a partition table already checked by `flash::check_partition_table`.
pub async fn load(flash: &mut FlashStorage<'static>) -> Result<Option<DeviceConfig>, &'static str> {
    let mut storage = YieldingAsync::new(BlockingAsync::new(flash));
    let result = config_store::load(&mut storage, device_config::PARTITION)
        .await
        .map_err(|_| "config_storage")?;
    GENERATION.store(
        result.as_ref().map_or(0, |cfg| cfg.generation),
        Ordering::Relaxed,
    );
    #[cfg(feature = "coap")]
    if let Some(config) = &result {
        set_device_credential(config);
    }
    Ok(result)
}

fn respond(id: &str, status: &str, generation: u32) {
    esp_println::println!(
        "@CONFIG {{\"id\":\"{}\",\"status\":\"{}\",\"schema\":{},\"generation\":{}{}}}",
        id,
        status,
        device_config::SCHEMA,
        generation,
        device_credential_fields()
    );
}

#[embassy_executor::task]
pub async fn usb_task(mut rx: UsbSerialJtagRx<'static, Async>) {
    let mut frame = [0; provisioning::MAX_FRAME];
    let mut count = 0;
    let mut overflow = false;
    let mut chunk = [0; 64];
    loop {
        let read = match rx.read(&mut chunk).await {
            Ok(read) => read,
            Err(_) => {
                Timer::after_millis(100).await;
                continue;
            }
        };
        for &byte in &chunk[..read] {
            if byte == b'\n' {
                let generation = GENERATION.load(Ordering::Relaxed);
                if overflow {
                    respond("", "too_large", generation);
                } else {
                    match Command::decode(&frame[..count]) {
                        Ok(Command::Status { id }) => respond(
                            &id,
                            if generation == 0 {
                                "unconfigured"
                            } else {
                                "configured"
                            },
                            generation,
                        ),
                        Ok(command) if flash::try_claim() => flash::queue(Job::Config(command)),
                        Ok(command) => respond(command.id(), "busy", generation),
                        Err(_) => respond("", "invalid", generation),
                    }
                }
                // Do not leave a second copy of provisioning credentials in the RX frame.
                frame.fill(0);
                count = 0;
                overflow = false;
            } else if count < frame.len() {
                frame[count] = byte;
                count += 1;
            } else {
                overflow = true;
            }
        }
        chunk.fill(0);
    }
}

/// The first time a device is given client keys, it also needs a key of its own.
#[cfg(feature = "coap")]
fn provide_device_key(mut config: DeviceConfig) -> Result<DeviceConfig, config_store::Error> {
    use esp_hal::rng::Trng;
    use esp32_led_mqtt::coap_keys;

    if !config.coap_clients.is_empty() && config.coap_device_key.is_empty() {
        let rng = Trng::try_new().map_err(|_| config_store::Error::Storage)?;
        config.coap_device_key =
            coap_keys::generate_device_key(rng).map_err(|_| config_store::Error::Storage)?;
    }
    set_device_credential(&config);
    Ok(config)
}

/// Persists a USB write command and reports the result. The caller must reboot
/// afterwards, even after an error.
pub async fn handle(flash: &mut FlashStorage<'static>, command: Command) {
    let mut storage = YieldingAsync::new(BlockingAsync::new(flash));
    let range = device_config::PARTITION;
    let (id, result) = match command {
        Command::Configure { id, mut config } => {
            let saved = match config_store::load(&mut storage, range.clone()).await {
                Ok(stored) => {
                    config.keep_coap_keys(stored.as_ref());
                    #[cfg(feature = "coap")]
                    let config = provide_device_key(config);
                    #[cfg(not(feature = "coap"))]
                    let config = Ok(config);
                    match config {
                        Ok(config) => config_store::save(&mut storage, range, config).await,
                        Err(error) => Err(error),
                    }
                }
                Err(error) => Err(error),
            };
            (id, saved)
        }
        Command::Erase { id } => (
            id,
            config_store::erase(&mut storage, range).await.map(|()| 0),
        ),
        Command::Status { .. } => unreachable!("the USB task answers status requests"),
    };
    match result {
        Ok(generation) => respond(
            &id,
            if generation == 0 { "erased" } else { "saved" },
            generation,
        ),
        Err(_) => respond(&id, "storage_error", GENERATION.load(Ordering::Relaxed)),
    }
}
