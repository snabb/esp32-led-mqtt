//! The logger for builds with CoAP: USB output as usual, plus a copy in a RAM ring.

use core::{cell::RefCell, fmt::Write};

use critical_section::Mutex;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use esp32_led_mqtt::log_buffer::{LOG_BYTES, LogFilter, LogRead, LogRing};
use log::{Metadata, Record};

/// Longest line kept. A longer one is cut short here, but not on USB.
const MAX_LINE: usize = 240;
const FILTER: LogFilter<'static> = LogFilter(match option_env!("ESP_LOG") {
    Some(rules) => rules,
    None => "info",
});

static RING: Mutex<RefCell<LogRing<LOG_BYTES>>> = Mutex::new(RefCell::new(LogRing::new()));
/// Raised for every line kept, which can be from an interrupt.
pub static LOGGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

struct Line {
    bytes: [u8; MAX_LINE],
    len: usize,
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let count = text.len().min(MAX_LINE - self.len);
        self.bytes[self.len..self.len + count].copy_from_slice(&text.as_bytes()[..count]);
        self.len += count;
        Ok(())
    }
}

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        FILTER.enabled(metadata.level(), metadata.target())
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        esp_println::println!("{} - {}", record.level(), record.args());

        // Format outside the critical section: arguments can take a while, and logging
        // can happen with interrupts enabled or from an interrupt.
        let mut line = Line {
            bytes: [0; MAX_LINE],
            len: 0,
        };
        let millis = esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_millis();
        let _ = write!(
            line,
            "[{}.{:03}] {} - {}",
            millis / 1000,
            millis % 1000,
            record.level(),
            record.args()
        );
        // Whether or not it was cut short, each line must end.
        line.len = line.len.min(MAX_LINE - 1);
        line.bytes[line.len] = b'\n';
        critical_section::with(|cs| {
            RING.borrow_ref_mut(cs).push(&line.bytes[..=line.len]);
        });
        LOGGED.signal(());
    }

    fn flush(&self) {}
}

static LOGGER: Logger = Logger;

pub fn init() {
    log::set_logger(&LOGGER).expect("the logger is set once");
    log::set_max_level(FILTER.max_level());
}

pub fn read(from: Option<u64>, output: &mut [u8]) -> LogRead {
    critical_section::with(|cs| RING.borrow_ref(cs).read(from, output))
}
