//! Sole owner of the flash. After the boot health check it runs one settings write
//! or OTA update at a time, or lends the flash to one firmware upload.

use core::{
    cell::RefCell,
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_net::iface::Iface;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Timer;
use esp_bootloader_esp_idf::partitions;
use esp_hal::{
    Blocking,
    flash::Flash,
    peripherals::{FLASH, TIMG0},
    timer::timg::{MwdtStage, MwdtStageAction, Wdt},
};
#[cfg(feature = "_control")]
use esp32_led_mqtt::ota_protocol;
use esp32_led_mqtt::word_flash::{ByteFlash, WORD_SIZE, WordFlash};
use esp32_led_mqtt::{device_config, provisioning::Command};

use crate::{config, ota};

#[allow(
    clippy::large_enum_variant,
    reason = "one bounded in-flight job owns its data without heap allocation"
)]
pub enum Job {
    #[cfg(feature = "_control")]
    Ota(ota_protocol::Request),
    Config(Command),
    #[cfg(feature = "coap")]
    CoapClients(esp32_led_mqtt::coap_api::ClientList),
    #[cfg(feature = "coap")]
    StoredConfig(device_config::DeviceConfig),
    #[cfg(feature = "coap")]
    Reboot,
    /// A firmware upload holds a `Lease`. This job only supervises it.
    #[cfg(feature = "coap")]
    SuperviseUpload,
}

/// Set while the owner can accept a job. Stays clear after an unhealthy boot.
static IDLE: AtomicBool = AtomicBool::new(false);
static JOB: Signal<CriticalSectionRawMutex, Job> = Signal::new();

/// Claims the single job slot; on success the caller must `queue` exactly one job.
pub fn try_claim() -> bool {
    IDLE.compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

pub fn queue(job: Job) {
    JOB.signal(job);
}

pub type FlashStorage = ByteFlash<EspFlash>;

/// Holds the flash while no job has it.
static STORAGE: critical_section::Mutex<RefCell<Option<FlashStorage>>> =
    critical_section::Mutex::new(RefCell::new(None));

fn take_storage() -> FlashStorage {
    critical_section::with(|cs| STORAGE.borrow_ref_mut(cs).take())
        .expect("only the job holder takes the storage")
}

fn put_storage(flash: FlashStorage) {
    critical_section::with(|cs| *STORAGE.borrow_ref_mut(cs) = Some(flash));
}

/// What a firmware upload tells the flash task.
#[cfg(feature = "coap")]
pub enum UploadEvent {
    Progress,
    Finished { reboot: bool },
}

#[cfg(feature = "coap")]
pub static UPLOAD_EVENT: Signal<CriticalSectionRawMutex, UploadEvent> = Signal::new();
/// A stalled upload times out in the CoAP server sooner than this.
#[cfg(feature = "coap")]
const UPLOAD_WATCHDOG_SECONDS: u64 = 60;

/// Exclusive use of the flash across requests, for a firmware upload. Dropping it returns
/// the storage; the holder must then send `UploadEvent::Finished` to release the job slot.
#[cfg(feature = "coap")]
pub struct Lease(Option<FlashStorage>);

/// Takes the flash, if no job is using it.
#[cfg(feature = "coap")]
pub fn try_lease() -> Option<Lease> {
    if !try_claim() {
        return None;
    }
    queue(Job::SuperviseUpload);
    Some(Lease(Some(take_storage())))
}

#[cfg(feature = "coap")]
impl core::borrow::Borrow<FlashStorage> for Lease {
    fn borrow(&self) -> &FlashStorage {
        self.0.as_ref().expect("present until dropped")
    }
}

#[cfg(feature = "coap")]
impl core::borrow::BorrowMut<FlashStorage> for Lease {
    fn borrow_mut(&mut self) -> &mut FlashStorage {
        self.0.as_mut().expect("present until dropped")
    }
}

#[cfg(feature = "coap")]
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(flash) = self.0.take() {
            put_storage(flash);
        }
    }
}

pub fn arm_watchdog(wdt: &mut Wdt<TIMG0<'static>>, seconds: u64) {
    wdt.set_timeout(
        MwdtStage::Stage0,
        esp_hal::time::Duration::from_secs(seconds),
    );
    wdt.set_stage_action(MwdtStage::Stage0, MwdtStageAction::ResetSystem);
    wdt.feed();
    wdt.enable();
}

/// esp-hal's flash driver for `ByteFlash`, refusing writes and erases in `running`.
pub struct EspFlash {
    flash: Flash<'static, Blocking>,
    /// The booted app slot, which the MMU maps for code and read-only data.
    running: Range<u32>,
}

#[derive(Debug)]
pub enum EspFlashError {
    Driver(#[expect(dead_code, reason = "kept for Debug output")] esp_hal::flash::Error),
    RunningImage,
}

impl EspFlash {
    fn writable(&self, from: u32, to: u32) -> Result<(), EspFlashError> {
        if from < self.running.end && self.running.start < to {
            return Err(EspFlashError::RunningImage);
        }
        Ok(())
    }
}

impl WordFlash for EspFlash {
    type Error = EspFlashError;

    fn capacity(&self) -> u32 {
        self.flash.capacity() as u32
    }

    fn read_words(&mut self, offset: u32, words: &mut [u32]) -> Result<(), EspFlashError> {
        self.flash
            .read(offset, words)
            .map_err(EspFlashError::Driver)
    }

    fn write_words(&mut self, offset: u32, words: &[u32]) -> Result<(), EspFlashError> {
        self.writable(offset, offset + words.len() as u32 * WORD_SIZE)?;
        // SAFETY: only the running app slot is mapped for instruction fetch or as
        // read-only data, and `writable` has excluded it.
        unsafe { self.flash.write(offset, words) }.map_err(EspFlashError::Driver)
    }

    fn erase_sectors(&mut self, from: u32, to: u32) -> Result<(), EspFlashError> {
        self.writable(from, to)?;
        // SAFETY: as in `write_words`.
        unsafe { self.flash.erase(from, to) }.map_err(EspFlashError::Driver)
    }
}

/// Takes the flash and checks the partitions this firmware boots from or writes against
/// `partitions.csv`. If the table can't be read, the running slot is unknown, so the
/// returned storage refuses all writes.
pub fn open(peripheral: FLASH<'static>) -> (FlashStorage, Result<(), &'static str>) {
    let mut flash = Flash::new(peripheral, esp_hal::flash::Config::default())
        .expect("the flash chip is identified by the bootloader");
    let everything = 0..flash.capacity() as u32;
    let mut buffer = [0; 512];
    let (running, layout) = match partitions::read_partition_table(&mut flash, &mut buffer) {
        Ok(table) => {
            let running = match table.booted_partition() {
                Ok(Some(part)) => part.offset()..part.offset() + part.len(),
                _ => everything,
            };
            (running, check_layout(&table))
        }
        Err(_) => (everything, Err("partition_table")),
    };
    (ByteFlash(EspFlash { flash, running }), layout)
}

fn check_layout(table: &partitions::PartitionTable<'_>) -> Result<(), &'static str> {
    use esp_bootloader_esp_idf::partitions::{
        AppPartitionSubType as App, DataPartitionSubType as Data, PartitionType as Type,
    };
    use esp32_led_mqtt::ota_protocol::SLOT_SIZE;
    let config = device_config::PARTITION;
    let expected = [
        (Type::App(App::Ota0), 0x10000, SLOT_SIZE),
        (Type::App(App::Ota1), 0x200000, SLOT_SIZE),
        (Type::Data(Data::Ota), 0xd000, 0x2000),
        (
            Type::Data(Data::Undefined),
            config.start,
            config.end - config.start,
        ),
    ];
    for (kind, offset, size) in expected {
        let part = table
            .find_partition(kind)
            .map_err(|_| "partition_table")?
            .ok_or("missing_partition")?;
        if part.offset() != offset || part.len() != size {
            return Err("partition_layout");
        }
    }
    // OTA alternates between exactly two slots; there is no factory image.
    if table
        .iter()
        .filter(|p| matches!(p.partition_type(), Type::App(_)))
        .count()
        != 2
    {
        return Err("partition_layout");
    }
    Ok(())
}

#[embassy_executor::task]
pub async fn task(
    iface: Option<Iface<'static>>,
    flash: FlashStorage,
    mut wdt: Wdt<TIMG0<'static>>,
    layout: Result<(), &'static str>,
    download: ota::DownloadContext,
    control_required: bool,
) {
    let mut flash = flash;
    let confirmed = ota::confirm_boot(iface, &mut flash, layout, control_required).await;
    wdt.disable();
    if confirmed.is_err() {
        return;
    }
    put_storage(flash);
    #[cfg(feature = "_control")]
    let client = ota::client(iface, download);
    #[cfg(not(feature = "_control"))]
    let () = download;
    // Without OTA jobs, the only job ends in a reset.
    #[cfg_attr(not(feature = "_control"), allow(clippy::never_loop))]
    loop {
        IDLE.store(true, Ordering::Release);
        match JOB.wait().await {
            #[cfg(feature = "_control")]
            Job::Ota(request) => {
                // Submissions require ota::AVAILABLE, which implies a client.
                if let Some(client) = &client {
                    let mut flash = take_storage();
                    // Backs up ota::UPDATE_TIMEOUT across synchronous flash operations.
                    arm_watchdog(&mut wdt, ota::DOWNLOAD_WATCHDOG_SECONDS);
                    ota::update(client, &mut flash, &request).await;
                    wdt.disable();
                    put_storage(flash);
                }
            }
            Job::Config(command) => {
                let mut flash = take_storage();
                arm_watchdog(&mut wdt, 30);
                config::handle(&mut flash, command).await;
                // Reboot even after an error: a failed write may have committed,
                // and active settings must match the stored ones.
                Timer::after_millis(250).await;
                esp_hal::system::software_reset();
            }
            #[cfg(feature = "coap")]
            Job::StoredConfig(settings) => {
                let mut flash = take_storage();
                arm_watchdog(&mut wdt, 30);
                config::save_stored_config(&mut flash, settings).await;
                wdt.disable();
                put_storage(flash);
            }
            #[cfg(feature = "coap")]
            Job::CoapClients(clients) => {
                let mut flash = take_storage();
                arm_watchdog(&mut wdt, 30);
                crate::coap::save_clients(&mut flash, clients).await;
                wdt.disable();
                put_storage(flash);
            }
            #[cfg(feature = "coap")]
            Job::Reboot => {
                // Lets the reply leave.
                Timer::after_millis(250).await;
                esp_hal::system::software_reset();
            }
            #[cfg(feature = "coap")]
            Job::SuperviseUpload => {
                arm_watchdog(&mut wdt, UPLOAD_WATCHDOG_SECONDS);
                let reboot = loop {
                    match UPLOAD_EVENT.wait().await {
                        UploadEvent::Progress => wdt.feed(),
                        UploadEvent::Finished { reboot } => break reboot,
                    }
                };
                if reboot {
                    // Lets the final reply leave.
                    Timer::after_secs(1).await;
                    esp_hal::system::software_reset();
                }
                wdt.disable();
            }
        }
    }
}
