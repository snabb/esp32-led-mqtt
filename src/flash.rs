//! Sole owner of `FlashStorage`. After the boot health check it runs one USB
//! configuration write or OTA update at a time.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_net::Stack;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Timer;
use esp_hal::{
    peripherals::TIMG0,
    timer::timg::{MwdtStage, MwdtStageAction, Wdt},
};
use esp_storage::FlashStorage;
use esp32_led_mqtt::{device_config, ota_protocol, provisioning::Command};

use crate::{config, ota};

pub enum Job {
    Ota(ota_protocol::Request),
    Config(Command),
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
    let config = device_config::PARTITION;
    let expected = [
        (Type::App(App::Ota0), 0x10000, ota_protocol::SLOT_SIZE),
        (Type::App(App::Ota1), 0x200000, ota_protocol::SLOT_SIZE),
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
    mut flash: FlashStorage<'static>,
    mut wdt: Wdt<TIMG0<'static>>,
    layout: Result<(), &'static str>,
    tls: crate::tls::Context,
) {
    let started = ota::start(stack, &mut flash, layout, tls).await;
    wdt.disable();
    let Ok(client) = started else {
        return;
    };
    loop {
        IDLE.store(true, Ordering::Release);
        match JOB.wait().await {
            Job::Ota(request) => {
                // Submissions require ota::AVAILABLE, which implies a client.
                if let Some(client) = &client {
                    // Backs up ota::UPDATE_TIMEOUT across synchronous flash operations.
                    arm_watchdog(&mut wdt, 190);
                    ota::update(client, &mut flash, &request).await;
                    wdt.disable();
                }
            }
            Job::Config(command) => {
                arm_watchdog(&mut wdt, 30);
                config::handle(&mut flash, command).await;
                // Reboot even after an error: a failed write may have committed,
                // and active settings must match the stored ones.
                Timer::after_millis(250).await;
                esp_hal::system::software_reset();
            }
        }
    }
}
