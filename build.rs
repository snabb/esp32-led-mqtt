use std::env;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    let firmware_target = target.starts_with("riscv32imac-unknown-none-elf");
    emit_build_identity();
    emit_hardware();

    if !firmware_target {
        return;
    }

    linker_be_nice();
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

fn emit_build_identity() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let seconds = env::var("SOURCE_DATE_EPOCH")
        .map(|value| {
            value
                .parse::<u64>()
                .expect("SOURCE_DATE_EPOCH must be Unix seconds")
        })
        .unwrap_or_else(|_| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("build clock before Unix epoch")
                .as_secs()
        });
    assert!(
        (1_767_225_600..=4_102_444_800).contains(&seconds),
        "build time must be between 2026 and 2100"
    );
    println!("cargo:rustc-env=FIRMWARE_BUILD_UNIX={seconds}");
    let date = time::OffsetDateTime::from_unix_timestamp(seconds as i64).expect("valid build time");
    println!(
        "cargo:rustc-env=FIRMWARE_BUILD_TIME={:02}:{:02}:{:02}",
        date.hour(),
        date.minute(),
        date.second()
    );
    println!(
        "cargo:rustc-env=FIRMWARE_BUILD_DATE={:04}-{:02}-{:02}",
        date.year(),
        date.month() as u8,
        date.day()
    );
    println!("cargo:rerun-if-env-changed=OTA_BUILD_ID");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.lock");
    for name in [
        "OTA_ALLOWED_HOSTS",
        "OTA_ALLOW_HTTP",
        "OTA_ROLLBACK_BOOTLOADER",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|value| value.trim().to_owned())
    };
    for name in ["HEAD", "index"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(&["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={path}");
    }
    let revision = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "nogit000".to_owned());
    let short = &revision[..revision.len().min(8)];
    let dirty = git(&["status", "--porcelain"]).is_none_or(|status| !status.is_empty());
    let stamp = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        date.year(),
        date.month() as u8,
        date.day(),
        date.hour(),
        date.minute(),
        date.second()
    );
    let id = env::var("OTA_BUILD_ID")
        .unwrap_or_else(|_| format!("{stamp}-{short}{}", if dirty { "-dirty" } else { "" }));
    assert!(
        !id.is_empty()
            && id.len() <= 31
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "OTA_BUILD_ID must be 1-31 ASCII letters, digits, dots, underscores or hyphens"
    );
    println!("cargo:rustc-env=FIRMWARE_BUILD_ID={id}");
}

fn emit_hardware() {
    use sha2::{Digest, Sha256};
    println!("cargo:rerun-if-changed=hardware.json");
    let bytes = std::fs::read("hardware.json").expect("hardware.json");
    let profile: serde_json::Value = serde_json::from_slice(&bytes).expect("hardware JSON");
    let text = |name: &str| profile[name].as_str().expect("hardware string field");
    let number = |name: &str| profile[name].as_u64().expect("hardware integer field");
    let board = text("board");
    assert!(
        !board.is_empty()
            && board
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "invalid board name"
    );
    let count = number("led_count");
    assert!((1..=512).contains(&count), "led_count must be 1-512");
    let timing = match text("timing") {
        "sk6812" => "SK68XX_TIMING",
        "ws2812" => "WS2812_TIMING",
        "ws2812b" => "WS2812B_TIMING",
        "ws2811" => "WS2811_TIMING",
        "ws2811_low_speed" => "WS2811_LOW_SPEED_TIMING",
        _ => panic!("unsupported LED timing"),
    };
    let order = match text("color_order") {
        "rgb" => "Rgb",
        "rbg" => "Rbg",
        "grb" => "Grb",
        "gbr" => "Gbr",
        "brg" => "Brg",
        "bgr" => "Bgr",
        _ => panic!("unsupported color order"),
    };
    let data = number("data_gpio");
    assert!(data <= 30);
    let optional_pin = |name: &str| {
        if profile[name].is_null() {
            (
                "none".to_owned(),
                "None::<esp_hal::gpio::AnyPin<'static>>".to_owned(),
            )
        } else {
            let pin = number(name);
            assert!(pin <= 30);
            (pin.to_string(), format!("Some($p.GPIO{pin}.into())"))
        }
    };
    let (button_id, button) = optional_pin("button_gpio");
    let (power_id, power) = optional_pin("power_gpio");
    let active_high = profile["power_active_high"]
        .as_bool()
        .expect("power_active_high");
    let level = if active_high { "High" } else { "Low" };
    let frame = number("frame_interval_us");
    assert!((1_000..=1_000_000).contains(&frame));
    let id = format!(
        "{board}-{}-{}-{count}led-d{data}-b{button_id}-p{power_id}{}",
        text("timing"),
        text("color_order"),
        if !active_high && power_id != "none" {
            "low"
        } else {
            ""
        }
    );
    let digest = Sha256::digest(&bytes);
    let hash: String = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    println!("cargo:rustc-env=FIRMWARE_APP_PROJECT=led-mqtt-{hash}");
    println!("cargo:rustc-env=FIRMWARE_HARDWARE_ID={id}");
    let generated = format!(
        r#"
const LED_COUNT: usize = {count};
const LED_POWER_ACTIVE_LEVEL: esp_hal::gpio::Level = esp_hal::gpio::Level::{level};
const LED_FRAME_INTERVAL: embassy_time::Duration = embassy_time::Duration::from_micros({frame});
const LED_TIMING: esp_hal_smartled::Timing = esp_hal_smartled::{timing};
type LedStrip<'d> = esp_hal_smartled::RmtSmartLeds<'d, {{ esp_hal_smartled::buffer_size::<smart_leds::RGB8>(LED_COUNT) }}, esp_hal::Blocking, smart_leds::RGB8, esp_hal_smartled::color_order::{order}>;
macro_rules! led_data_pin {{ ($p:ident) => {{ $p.GPIO{data} }}; }}
macro_rules! led_power_pin {{ ($p:ident) => {{ {power} }}; }}
macro_rules! button_pin {{ ($p:ident) => {{ {button} }}; }}
"#
    );
    let output = std::path::PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(output.join("hardware.rs"), generated).expect("write hardware config");
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
