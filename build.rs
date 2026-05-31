use std::{collections::BTreeMap, env, fs, path::Path};

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    let firmware_target = target.starts_with("riscv32imac-unknown-none-elf");
    emit_secrets_config(firmware_target);

    if !firmware_target {
        return;
    }

    linker_be_nice();
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

fn emit_secrets_config(require_secrets: bool) {
    println!("cargo:rerun-if-changed=secrets.yaml");
    println!("cargo:rerun-if-changed=src/secrets.rs");

    let generated = if Path::new("secrets.yaml").exists() {
        generate_from_yaml(Path::new("secrets.yaml"))
    } else {
        read_rust_secrets(Path::new("src/secrets.rs"), require_secrets)
    };

    let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo");
    fs::write(Path::new(&out_dir).join("secrets.rs"), generated)
        .expect("failed to write generated secrets config");
}

fn generate_from_yaml(path: &Path) -> String {
    let secrets = read_secrets(path);
    let mqtt_port = secrets
        .get("mqtt_port")
        .map(|value| {
            value
                .parse::<u16>()
                .unwrap_or_else(|_| panic!("secrets.yaml mqtt_port must be a u16"))
        })
        .unwrap_or(1883);

    format!(
        "pub const WIFI_SSID: &str = {:?};\n\
         pub const WIFI_PASSWORD: &str = {:?};\n\
         pub const MQTT_BROKER: &str = {:?};\n\
         pub const MQTT_PORT: u16 = {};\n\
         pub const MQTT_USERNAME: &str = {:?};\n\
         pub const MQTT_PASSWORD: &str = {:?};\n",
        required(&secrets, "wifi_ssid"),
        required(&secrets, "wifi_password"),
        required(&secrets, "mqtt_broker"),
        mqtt_port,
        secrets
            .get("mqtt_username")
            .map(String::as_str)
            .unwrap_or(""),
        secrets
            .get("mqtt_password")
            .map(String::as_str)
            .unwrap_or(""),
    )
}

fn read_rust_secrets(path: &Path, require_secrets: bool) -> String {
    let contents =
        fs::read_to_string(path).unwrap_or_else(|_| panic!("missing {}", path.display()));
    if require_secrets
        && (contents.contains("your-wifi-ssid") || contents.contains("your-wifi-password"))
    {
        panic!("edit src/secrets.rs or create secrets.yaml with Wi-Fi and MQTT settings");
    }
    contents
}

fn read_secrets(path: &Path) -> BTreeMap<String, String> {
    let contents =
        fs::read_to_string(path).unwrap_or_else(|_| panic!("failed to read {}", path.display()));
    let mut values = BTreeMap::new();

    for line in contents.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.starts_with('#') || key.contains(char::is_whitespace) {
            continue;
        }

        let value = strip_inline_comment(value.trim());
        let value = unquote(value.trim());
        values.insert(key.to_owned(), value.to_owned());
    }

    values
}

fn strip_inline_comment(value: &str) -> &str {
    match value.find(" #") {
        Some(index) => &value[..index],
        None => value,
    }
}

fn unquote(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return &value[1..value.len() - 1];
        }
    }

    value
}

fn required<'a>(values: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    let value = values
        .get(key)
        .unwrap_or_else(|| panic!("secrets.yaml is missing required key {key}"));
    if value.is_empty() {
        panic!("secrets.yaml key {key} must not be empty");
    }
    value
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
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
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
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
