mod tls;

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::{
    select::{Either, select},
    yield_now,
};
use embassy_net::{IpAddress, IpEndpoint, Stack, dns::DnsQueryType, tcp::TcpSocket};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::{Read, Write};
use esp_hal::{
    peripherals::TIMG0,
    timer::timg::{MwdtStage, MwdtStageAction, Wdt},
};
use esp_hal_ota::{Ota, OtaImgState};
use esp_storage::FlashStorage;
use esp32_led_mqtt::ota_protocol::{self, Request, Verifier};
use heapless::String;
use log::{info, warn};

pub static LED_READY: AtomicBool = AtomicBool::new(false);
pub static MQTT_READY: AtomicBool = AtomicBool::new(false);
pub static STATUS: Signal<CriticalSectionRawMutex, String<192>> = Signal::new();
static REQUEST: Signal<CriticalSectionRawMutex, Request> = Signal::new();
static AVAILABLE: AtomicBool = AtomicBool::new(false);
static BUSY: AtomicBool = AtomicBool::new(false);
const READ_TIMEOUT: Duration = Duration::from_secs(15);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(180);
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
type Updater<'a> = Ota<FlashAccess<'a>>;

pub fn busy() -> bool {
    BUSY.load(Ordering::Acquire)
}

pub fn arm_watchdog(wdt: &mut Wdt<TIMG0<'static>>) {
    wdt.set_timeout(MwdtStage::Stage0, esp_hal::time::Duration::from_secs(125));
    wdt.set_stage_action(MwdtStage::Stage0, MwdtStageAction::ResetSystem);
    wdt.enable();
}

pub fn submit(payload: &[u8], retained: bool) {
    let result = (|| {
        if retained {
            return Err("retained_command");
        }
        if !AVAILABLE.load(Ordering::Acquire) {
            return Err("unavailable");
        }
        let request = Request::parse(
            payload,
            option_env!("OTA_ALLOWED_HOSTS").unwrap_or(""),
            option_env!("OTA_ALLOW_HTTP") == Some("1"),
        )
        .map_err(|_| "invalid_command")?;
        if request.build == env!("FIRMWARE_BUILD_ID") {
            return Err("already_running");
        }
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "busy")?;
        REQUEST.signal(request);
        Ok(())
    })();
    if let Err(reason) = result {
        status("rejected", reason, 0);
    }
}

fn status(state: &str, detail: &str, percent: u8) {
    let value = heapless::format!(
        "{{\"state\":\"{}\",\"detail\":\"{}\",\"progress\":{},\"running_build\":\"{}\"}}",
        state,
        detail,
        percent,
        env!("FIRMWARE_BUILD_ID")
    )
    .expect("bounded OTA status");
    STATUS.signal(value);
}

#[embassy_executor::task]
pub async fn task(
    stack: Option<Stack<'static>>,
    mut flash: FlashStorage<'static>,
    mut wdt: Wdt<TIMG0<'static>>,
) {
    let health = boot_health(stack, &mut flash).await;
    wdt.disable();
    if let Err(reason) = health {
        warn!("OTA unavailable: {}", reason);
        status("disabled", reason, 0);
    }
    crate::config::WRITES_ALLOWED.store(health.is_ok(), Ordering::Release);
    let tls = if health.is_ok()
        && stack.is_some()
        && option_env!("OTA_ROLLBACK_BOOTLOADER") == Some("1")
        && !option_env!("OTA_ALLOWED_HOSTS").unwrap_or("").is_empty()
    {
        tls::initialize().ok()
    } else {
        None
    };
    AVAILABLE.store(tls.is_some(), Ordering::Release);
    if tls.is_some() {
        status("ready", "", 0);
    } else if health.is_ok() {
        status("disabled", "configure_network_and_ota", 0);
    }
    loop {
        match select(REQUEST.wait(), crate::config::REQUEST.wait()).await {
            Either::First(request) => {
                let (Some(stack), Some(tls)) = (stack, tls) else {
                    BUSY.store(false, Ordering::Release);
                    continue;
                };
                crate::config::WRITES_ALLOWED.store(false, Ordering::Release);
                status("downloading", "", 0);
                wdt.set_timeout(MwdtStage::Stage0, esp_hal::time::Duration::from_secs(190));
                wdt.feed();
                wdt.enable();
                let deadline = Instant::now() + UPDATE_TIMEOUT;
                let result = match Ota::new(FlashAccess(&mut flash)) {
                    Ok(mut ota) => {
                        with_timeout(
                            UPDATE_TIMEOUT,
                            download(tls, stack, &mut ota, &request, deadline),
                        )
                        .await
                    }
                    Err(_) => Ok(Err("partition_table")),
                };
                match result {
                    Ok(Ok(())) => {
                        AVAILABLE.store(false, Ordering::Release);
                        status("rebooting", "verified", 100);
                        Timer::after_secs(1).await;
                        esp_hal::system::software_reset();
                    }
                    Ok(Err(reason)) => {
                        warn!("OTA failed: {}", reason);
                        status("failed", reason, 0);
                    }
                    Err(_) => {
                        warn!("OTA timed out");
                        status("failed", "timeout", 0);
                    }
                }
                wdt.disable();
                BUSY.store(false, Ordering::Release);
                crate::config::WRITES_ALLOWED.store(true, Ordering::Release);
            }
            Either::Second(command) => {
                crate::config::WRITES_ALLOWED.store(false, Ordering::Release);
                wdt.set_timeout(MwdtStage::Stage0, esp_hal::time::Duration::from_secs(30));
                wdt.feed();
                wdt.enable();
                if crate::config::handle(&mut flash, command).await {
                    Timer::after_millis(250).await;
                    esp_hal::system::software_reset();
                }
                wdt.disable();
                crate::config::WRITES_ALLOWED.store(health.is_ok(), Ordering::Release);
            }
        }
    }
}

async fn boot_health(
    stack: Option<Stack<'static>>,
    flash: &mut FlashStorage<'static>,
) -> Result<(), &'static str> {
    let start = Instant::now();
    initialize(flash)?;
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
                MQTT_READY.load(Ordering::Acquire),
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

fn initialize(flash: &mut FlashStorage<'static>) -> Result<(), &'static str> {
    use esp_bootloader_esp_idf::partitions::{
        self, AppPartitionSubType as App, DataPartitionSubType as Data, PartitionType as Type,
    };
    let mut buffer = [0; 512];
    let table =
        partitions::read_partition_table(flash, &mut buffer).map_err(|_| "partition_table")?;
    let expected = [
        (Type::App(App::Ota0), 0x10000, ota_protocol::SLOT_SIZE),
        (Type::App(App::Ota1), 0x200000, ota_protocol::SLOT_SIZE),
        (Type::Data(Data::Ota), 0xd000, 0x2000),
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

async fn download(
    tls_context: &mbedtls_rs::Tls<'static>,
    stack: Stack<'static>,
    ota: &mut Updater<'_>,
    request: &Request,
    deadline: Instant,
) -> Result<(), &'static str> {
    let address = match request.host.parse::<core::net::Ipv4Addr>() {
        Ok(address) => IpAddress::Ipv4(address),
        Err(_) => {
            let addresses = with_timeout(
                READ_TIMEOUT,
                stack.dns_query(&request.host, DnsQueryType::A),
            )
            .await
            .map_err(|_| "dns_timeout")?
            .map_err(|_| "dns")?;
            let Some(embassy_net::dns::IpAddress::Ipv4(address)) = addresses.first() else {
                return Err("dns");
            };
            IpAddress::Ipv4(*address)
        }
    };
    let mut rx = [0; 4096];
    let mut tx = [0; 2048];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    socket.set_timeout(Some(READ_TIMEOUT));
    with_timeout(
        READ_TIMEOUT,
        socket.connect(IpEndpoint::new(address, request.port)),
    )
    .await
    .map_err(|_| "connect_timeout")?
    .map_err(|_| "connect")?;
    if request.tls {
        tls::transfer(tls_context, socket, ota, request, deadline).await
    } else {
        transfer(&mut socket, ota, request, deadline).await
    }
}

async fn transfer<IO: Read + Write>(
    io: &mut IO,
    ota: &mut Updater<'_>,
    request: &Request,
    deadline: Instant,
) -> Result<(), &'static str> {
    let get: String<768> = heapless::format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        request.path,
        request.host,
        request.port
    )
    .map_err(|_| "url")?;
    with_timeout(READ_TIMEOUT, io.write_all(get.as_bytes()))
        .await
        .map_err(|_| "request_timeout")?
        .map_err(|_| "request")?;
    with_timeout(READ_TIMEOUT, io.flush())
        .await
        .map_err(|_| "request_timeout")?
        .map_err(|_| "request")?;
    let mut headers = [0; 2048];
    let mut len = 0;
    loop {
        if len == headers.len() {
            return Err("headers_too_large");
        }
        read_exact(io, &mut headers[len..len + 1]).await?;
        len += 1;
        if headers[..len].ends_with(b"\r\n\r\n") {
            break;
        }
    }
    ota_protocol::validate_headers(&headers[..len], request.size).map_err(|_| "http_framing")?;
    let mut remaining = request.size as usize;
    let mut block = [0; 4096];
    let mut verifier = Verifier::new(request);
    let mut first = true;
    let mut last_percent = 0;
    while remaining != 0 {
        let count = remaining.min(block.len());
        read_exact(io, &mut block[..count]).await?;
        if first {
            ota_protocol::validate_image_prefix(&block[..count], &request.build)
                .map_err(|_| "image_identity")?;
            ota.begin(request.size, request.crc32)
                .map_err(|_| "begin")?;
            first = false;
        }
        verifier.update(&block[..count]).map_err(|_| "size")?;
        if Instant::now() >= deadline {
            return Err("timeout");
        }
        ota.write(&block[..count]).map_err(|_| "flash_write")?;
        remaining -= count;
        let percent = ((request.size as usize - remaining) * 100 / request.size as usize) as u8;
        if percent / 10 != last_percent / 10 {
            status("downloading", "", percent);
            last_percent = percent;
        }
        yield_now().await;
    }
    verifier.finish().map_err(|_| "sha256")?;
    status("verifying", "flash_readback", 100);
    if Instant::now() >= deadline {
        return Err("timeout");
    }
    ota.finish(true).map_err(|_| "verify_or_activate")?;
    Ok(())
}

async fn read_exact<IO: Read>(io: &mut IO, mut buffer: &mut [u8]) -> Result<(), &'static str> {
    while !buffer.is_empty() {
        let count = with_timeout(READ_TIMEOUT, io.read(buffer))
            .await
            .map_err(|_| "read_timeout")?
            .map_err(|_| "read")?;
        if count == 0 {
            return Err("truncated");
        }
        buffer = &mut buffer[count..];
    }
    Ok(())
}
