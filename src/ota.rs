use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::yield_now;
use embassy_net::{IpEndpoint, Stack, tcp::TcpSocket};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::{Read, Write};
use esp_hal::sha::Sha256Context;
use esp_hal_ota::{Ota, OtaImgState};
use esp_storage::FlashStorage;
use esp32_led_mqtt::ota_protocol::{self, Request, Verifier};
use heapless::String;
use log::{info, warn};
use mbedtls_rs::TlsGroup;

use crate::flash::{self, Job};

pub static LED_READY: AtomicBool = AtomicBool::new(false);
pub static MQTT_READY: AtomicBool = AtomicBool::new(false);
pub static STATUS: Signal<CriticalSectionRawMutex, String<192>> = Signal::new();
static AVAILABLE: AtomicBool = AtomicBool::new(false);
const READ_TIMEOUT: Duration = Duration::from_secs(15);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(180);
/// Only P-256 reaches the ECC accelerator, and it must be offered alone: with X25519
/// also listed, Google's servers pick X25519 even when P-256 comes first. TLS 1.3
/// requires servers to support P-256, so there is no fallback.
const KEY_EXCHANGE_GROUPS: &[TlsGroup] = &[TlsGroup::Secp256r1];
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

/// Network and TLS context for downloading updates. Holding the TLS context also
/// proves `tls::initialize` started the SHA queue, without which hashing hangs.
pub struct Client {
    stack: Stack<'static>,
    tls: &'static mbedtls_rs::Tls<'static>,
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
        if !flash::try_claim() {
            return Err("busy");
        }
        flash::queue(Job::Ota(request));
        Ok(())
    })();
    if let Err(reason) = result {
        status("rejected", reason, 0);
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
    STATUS.signal(value);
}

/// Whether this image was built with the settings required for OTA.
pub fn enabled() -> bool {
    option_env!("OTA_ROLLBACK_BOOTLOADER") == Some("1")
        && !option_env!("OTA_ALLOWED_HOSTS").unwrap_or("").is_empty()
}

/// Confirms or rolls back a trial image, then enables updates when configured.
/// Returns `Err` for an unhealthy boot, which must keep flash writes disabled.
pub async fn start(
    stack: Option<Stack<'static>>,
    flash: &mut FlashStorage<'static>,
    layout: Result<(), &'static str>,
    tls_context: crate::tls::Context,
) -> Result<Option<Client>, &'static str> {
    let health = match layout {
        Ok(()) => boot_health(stack, flash).await,
        Err(reason) => Err(reason),
    };
    if let Err(reason) = health {
        warn!("OTA unavailable: {}", reason);
        status("disabled", reason, 0);
        return Err(reason);
    }
    let (client, disabled_reason) = if enabled() {
        match (stack, tls_context) {
            (Some(stack), Ok(tls)) => (Some(Client { stack, tls }), ""),
            (Some(_), Err(reason)) => (None, reason),
            (None, _) => (None, "configure_network_and_ota"),
        }
    } else {
        (None, "configure_network_and_ota")
    };
    AVAILABLE.store(client.is_some(), Ordering::Release);
    if client.is_some() {
        status("ready", "", 0);
    } else {
        status("disabled", disabled_reason, 0);
    }
    Ok(client)
}

/// Downloads, verifies and activates `request`, rebooting into it on success.
pub async fn update(client: &Client, flash: &mut FlashStorage<'static>, request: &Request) {
    status("downloading", "", 0);
    let result = match Ota::new(FlashAccess(flash)) {
        Ok(mut ota) => with_timeout(
            UPDATE_TIMEOUT,
            download(client.tls, client.stack, &mut ota, request),
        )
        .await
        .unwrap_or(Err("timeout")),
        Err(_) => Err("partition_table"),
    };
    match result {
        Ok(()) => {
            AVAILABLE.store(false, Ordering::Release);
            status("rebooting", "verified", 100);
            Timer::after_secs(1).await;
            esp_hal::system::software_reset();
        }
        Err(reason) => {
            warn!("OTA failed: {}", reason);
            status("failed", reason, 0);
        }
    }
}

async fn boot_health(
    stack: Option<Stack<'static>>,
    flash: &mut FlashStorage<'static>,
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

async fn download(
    tls_context: &mbedtls_rs::Tls<'static>,
    stack: Stack<'static>,
    ota: &mut Updater<'_>,
    request: &Request,
) -> Result<(), &'static str> {
    let address = crate::resolve_ipv4(stack, &request.host).await?;
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
        let mut stream = crate::tls::connect(
            tls_context,
            crate::tls::Socket(socket),
            &request.host,
            Some(KEY_EXCHANGE_GROUPS),
        )
        .await?;
        transfer(&mut stream, ota, request).await
    } else {
        transfer(&mut socket, ota, request).await
    }
}

async fn transfer<IO: Read + Write>(
    io: &mut IO,
    ota: &mut Updater<'_>,
    request: &Request,
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
    let mut verifier = Verifier::<Sha256Context>::new(request);
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
