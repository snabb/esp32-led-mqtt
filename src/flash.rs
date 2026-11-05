//! Sole owner of `FlashStorage`. After the boot health check it runs one settings write
//! or OTA update at a time, or lends the flash to one firmware upload.

use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_net::Stack;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Timer;
use esp_hal::{
    peripherals::TIMG0,
    timer::timg::{MwdtStage, MwdtStageAction, Wdt},
};
use esp_storage::FlashStorage;
#[cfg(feature = "_control")]
use esp32_led_mqtt::ota_protocol;
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

/// Holds `FlashStorage` while no job has it.
static STORAGE: critical_section::Mutex<RefCell<Option<FlashStorage<'static>>>> =
    critical_section::Mutex::new(RefCell::new(None));

fn take_storage() -> FlashStorage<'static> {
    critical_section::with(|cs| STORAGE.borrow_ref_mut(cs).take())
        .expect("only the job holder takes the storage")
}

fn put_storage(flash: FlashStorage<'static>) {
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
pub struct Lease(Option<FlashStorage<'static>>);

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
impl core::borrow::Borrow<FlashStorage<'static>> for Lease {
    fn borrow(&self) -> &FlashStorage<'static> {
        self.0.as_ref().expect("present until dropped")
    }
}

#[cfg(feature = "coap")]
impl core::borrow::BorrowMut<FlashStorage<'static>> for Lease {
    fn borrow_mut(&mut self) -> &mut FlashStorage<'static> {
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

/// Checks the partitions this firmware boots from or writes against `partitions.csv`.
pub fn check_partition_table(flash: &mut FlashStorage<'static>) -> Result<(), &'static str> {
    use esp_bootloader_esp_idf::partitions::{
        self, AppPartitionSubType as App, DataPartitionSubType as Data, PartitionType as Type,
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
    let mut buffer = [0; 512];
    let table =
        partitions::read_partition_table(flash, &mut buffer).map_err(|_| "partition_table")?;
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
    stack: Option<Stack<'static>>,
    flash: FlashStorage<'static>,
    mut wdt: Wdt<TIMG0<'static>>,
    layout: Result<(), &'static str>,
    download: ota::DownloadContext,
    control_required: bool,
) {
    let mut flash = flash;
    let confirmed = ota::confirm_boot(stack, &mut flash, layout, control_required).await;
    wdt.disable();
    if confirmed.is_err() {
        return;
    }
    put_storage(flash);
    #[cfg(feature = "_control")]
    let client = ota::client(stack, download);
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
                    arm_watchdog(&mut wdt, 190);
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
