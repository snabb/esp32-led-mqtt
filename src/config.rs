use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use embassy_embedded_hal::adapter::{BlockingAsync, YieldingAsync};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Timer;
use embedded_io_async::Read;
use esp_hal::{Async, usb::usb_serial_jtag::UsbSerialJtagRx};
use esp_storage::FlashStorage;
use esp32_led_mqtt::{
    config_store,
    device_config::{self, DeviceConfig},
    provisioning::{self, Command},
};

pub static REQUEST: Signal<CriticalSectionRawMutex, Command> = Signal::new();
pub static WRITES_ALLOWED: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU32 = AtomicU32::new(0);

pub fn partition(flash: &mut FlashStorage<'static>) -> Result<core::ops::Range<u32>, &'static str> {
    use esp_bootloader_esp_idf::partitions;
    let mut buffer = [0; 512];
    let table =
        partitions::read_partition_table(flash, &mut buffer).map_err(|_| "partition_table")?;
    let found = table
        .iter()
        .find(|entry| entry.label_as_str() == "device_config")
        .ok_or("missing_config_partition")?;
    if found.offset() != device_config::PARTITION_OFFSET
        || found.len() != device_config::PARTITION_SIZE
        || found.raw_type() != 1
        || found.raw_subtype() != 0x06
    {
        return Err("config_partition_layout");
    }
    Ok(found.offset()..found.offset() + found.len())
}

pub async fn load(flash: &mut FlashStorage<'static>) -> Result<Option<DeviceConfig>, &'static str> {
    let range = partition(flash)?;
    let result = config_store::load(&mut YieldingAsync::new(BlockingAsync::new(flash)), range)
        .await
        .map_err(|_| "config_storage")?;
    GENERATION.store(
        result.as_ref().map_or(0, |cfg| cfg.generation),
        Ordering::Relaxed,
    );
    Ok(result)
}

fn respond(id: &str, status: &str, generation: u32) {
    esp_println::println!(
        "@CONFIG {{\"id\":\"{}\",\"status\":\"{}\",\"schema\":{},\"generation\":{}}}",
        id,
        status,
        device_config::SCHEMA,
        generation
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
                if !overflow {
                    match Command::decode(&frame[..count]) {
                        Ok(Command::Status { id }) => respond(
                            &id,
                            if GENERATION.load(Ordering::Relaxed) == 0 {
                                "unconfigured"
                            } else {
                                "configured"
                            },
                            GENERATION.load(Ordering::Relaxed),
                        ),
                        Ok(command) => {
                            if !WRITES_ALLOWED.load(Ordering::Acquire) || crate::ota::busy() {
                                respond(command.id(), "busy", GENERATION.load(Ordering::Relaxed));
                            } else if PENDING
                                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                                .is_ok()
                            {
                                REQUEST.signal(command);
                            } else {
                                respond(command.id(), "busy", GENERATION.load(Ordering::Relaxed));
                            }
                        }
                        Err(_) => respond("", "invalid", GENERATION.load(Ordering::Relaxed)),
                    }
                } else {
                    respond("", "too_large", GENERATION.load(Ordering::Relaxed));
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

/// Returns true when the caller must reboot to reconcile persistent and active settings.
pub async fn handle(flash: &mut FlashStorage<'static>, command: Command) -> bool {
    let id = heapless::String::<32>::try_from(command.id()).expect("validated request ID");
    let result = match partition(flash) {
        Ok(range) => {
            let mut storage = YieldingAsync::new(BlockingAsync::new(&mut *flash));
            match command {
                Command::Configure { config, .. } => {
                    config_store::save(&mut storage, range, config).await
                }
                Command::Erase { .. } => config_store::erase(&mut storage, range).await.map(|()| 0),
                Command::Status { .. } => unreachable!(),
            }
            .map_err(|_| "storage_error")
        }
        Err(reason) => Err(reason),
    };
    PENDING.store(false, Ordering::Release);
    match result {
        Ok(generation) => {
            GENERATION.store(generation, Ordering::Relaxed);
            respond(
                &id,
                if generation == 0 { "erased" } else { "saved" },
                generation,
            );
            true
        }
        Err(reason) => {
            // A flash error can occur after commit, including during readback or erase.
            // Reboot so active credentials cannot silently differ from persistent settings.
            if reason == "storage_error"
                && let Ok(range) = partition(flash)
            {
                let mut storage = YieldingAsync::new(BlockingAsync::new(&mut *flash));
                if let Ok(config) = config_store::load(&mut storage, range).await {
                    GENERATION.store(
                        config.map_or(0, |config| config.generation),
                        Ordering::Relaxed,
                    );
                }
            }
            respond(&id, reason, GENERATION.load(Ordering::Relaxed));
            reason == "storage_error"
        }
    }
}
