use core::ffi::CStr;

use embassy_net::tcp::TcpSocket;
use embassy_time::{Duration, Instant, with_timeout};
use esp_hal::rng::Trng;
use mbedtls_rs::{
    AuthMode, Certificate, ClientSessionConfig, Session, SessionConfig, Tls, X509,
    sys::{
        hook::{timer::MbedtlsTimer, wall_clock::MbedtlsWallClock},
        tm,
    },
};
use static_cell::StaticCell;

use super::{Request, Updater};

static RNG: StaticCell<Trng> = StaticCell::new();
static CONTEXT: StaticCell<Tls<'static>> = StaticCell::new();
static CLOCK: Clock = Clock;
struct Clock;
impl MbedtlsTimer for Clock {
    fn now(&self) -> i64 {
        Instant::now().as_millis() as i64
    }
}
impl MbedtlsWallClock for Clock {
    fn instant(&self) -> Option<tm> {
        let seconds = env!("FIRMWARE_BUILD_UNIX").parse::<i64>().ok()?;
        let now = time::OffsetDateTime::from_unix_timestamp(seconds).ok()?;
        Some(tm {
            tm_sec: now.second() as i32,
            tm_min: now.minute() as i32,
            tm_hour: now.hour() as i32,
            tm_mday: now.day() as i32,
            tm_mon: now.month() as i32 - 1,
            tm_year: now.year() - 1900,
            tm_wday: now.weekday().number_days_from_sunday() as i32,
            tm_yday: now.ordinal() as i32 - 1,
            tm_isdst: 0,
        })
    }
}

/// Install the global MbedTLS hooks once, before any TLS object is created.
pub fn initialize() -> Result<&'static Tls<'static>, &'static str> {
    // SAFETY: OTA owns all TLS contexts; these static hooks outlive every session.
    unsafe {
        mbedtls_rs::sys::hook::timer::hook_timer(Some(&CLOCK));
        mbedtls_rs::sys::hook::wall_clock::hook_wall_clock(Some(&CLOCK));
    }
    let rng = Trng::try_new().map_err(|_| "tls_entropy")?;
    let tls = Tls::new(RNG.init(rng)).map_err(|_| "tls_init")?;
    Ok(CONTEXT.init(tls))
}

pub async fn transfer(
    context: &Tls<'static>,
    socket: TcpSocket<'_>,
    ota: &mut Updater<'_>,
    request: &Request,
) -> Result<(), &'static str> {
    let pem = CStr::from_bytes_with_nul(concat!(include_str!("roots.pem"), "\0").as_bytes())
        .map_err(|_| "tls_roots")?;
    let ca = Certificate::new(X509::PEM(pem)).map_err(|_| "tls_roots")?;
    let mut hostname =
        heapless::String::<129>::try_from(request.host.as_str()).map_err(|_| "tls_host")?;
    hostname.push('\0').map_err(|_| "tls_host")?;
    let server = CStr::from_bytes_with_nul(hostname.as_bytes()).map_err(|_| "tls_host")?;
    let config = SessionConfig::Client(ClientSessionConfig {
        ca_chain: Some(ca),
        server_name: Some(server),
        // Dates are checked against the build-time lower bound. OPTIONAL permits
        // a renewed certificate's FUTURE flag; every other failure is rejected
        // below, before sending any application data.
        auth_mode: AuthMode::Optional,
        ..Default::default()
    });
    let mut tls = Session::new(context.reference(), socket, &config).map_err(|_| "tls_session")?;
    with_timeout(Duration::from_secs(30), tls.connect())
        .await
        .map_err(|_| "tls_timeout")?
        .map_err(|err| {
            log::warn!(
                "OTA TLS verification/handshake failed: {:?}, flags={:#x}",
                err,
                tls.tls_verification_details()
            );
            "tls_handshake"
        })?;
    if !esp32_led_mqtt::ota_protocol::tls_flags_acceptable(
        tls.tls_verification_details(),
        mbedtls_rs::sys::MBEDTLS_X509_BADCERT_FUTURE,
    ) {
        return Err("tls_certificate");
    }
    super::transfer(&mut tls, ota, request).await
}
