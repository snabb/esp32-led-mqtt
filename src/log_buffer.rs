//! Keeps recent log output in RAM for reading over CoAP.

use log::{Level, LevelFilter};

/// Bytes of log text kept. At a few hundred log lines this covers a whole boot.
pub const LOG_BYTES: usize = 8192;

/// A byte ring addressed by offsets that count every byte ever written, so a reader can
/// tell what it has missed.
pub struct LogRing<const N: usize> {
    bytes: [u8; N],
    written: u64,
}

/// What a read found. `start..next` are the offsets of the bytes copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogRead {
    pub start: u64,
    pub next: u64,
}

impl<const N: usize> LogRing<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            written: 0,
        }
    }

    pub fn push(&mut self, mut bytes: &[u8]) {
        self.written += bytes.len() as u64;
        // Only the tail of an oversized write can survive.
        if bytes.len() > N {
            bytes = &bytes[bytes.len() - N..];
        }
        let at = ((self.written - bytes.len() as u64) % N as u64) as usize;
        let (first, second) = bytes.split_at(bytes.len().min(N - at));
        self.bytes[at..at + first.len()].copy_from_slice(first);
        self.bytes[..second.len()].copy_from_slice(second);
    }

    /// Copies bytes from offset `from`, or from the oldest still held if those are gone.
    /// Without `from`, copies as much of the newest data as fits. Offsets in the future
    /// (a reader from before a reboot) read from the oldest data too.
    pub fn read(&self, from: Option<u64>, output: &mut [u8]) -> LogRead {
        let oldest = self.written.saturating_sub(N as u64);
        let newest_fit = self.written.saturating_sub(output.len() as u64).max(oldest);
        let start = match from {
            None => newest_fit,
            Some(from) if from < oldest || from > self.written => oldest,
            Some(from) => from,
        };
        let count = ((self.written - start) as usize).min(output.len());
        let at = (start % N as u64) as usize;
        let first = count.min(N - at);
        output[..first].copy_from_slice(&self.bytes[at..at + first]);
        output[first..count].copy_from_slice(&self.bytes[..count - first]);
        LogRead {
            start,
            next: start + count as u64,
        }
    }
}

impl<const N: usize> Default for LogRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// `ESP_LOG`-style rules such as `info,embassy_net=warn`: a bare level sets the default,
/// and the most specific matching `target=level` wins. Mirrors esp-println's filter, which
/// isn't public. Text that doesn't parse is ignored.
#[derive(Clone, Copy)]
pub struct LogFilter<'a>(pub &'a str);

fn parse_level(text: &str) -> Option<LevelFilter> {
    text.trim().parse().ok()
}

impl LogFilter<'_> {
    pub fn enabled(&self, level: Level, target: &str) -> bool {
        let mut limit = LevelFilter::Info;
        let mut matched = 0;
        let mut default_set = false;
        for rule in self.0.split(',') {
            match rule.split_once('=') {
                Some((prefix, rule_level)) => {
                    let prefix = prefix.trim();
                    let applies = target
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"));
                    if applies
                        && prefix.len() >= matched
                        && let Some(rule_level) = parse_level(rule_level)
                    {
                        limit = rule_level;
                        matched = prefix.len().max(1);
                    }
                }
                None => {
                    if matched == 0
                        && !default_set
                        && let Some(rule_level) = parse_level(rule)
                    {
                        limit = rule_level;
                        default_set = true;
                    }
                }
            }
        }
        level <= limit
    }

    /// The highest level any rule allows, for `log::set_max_level`.
    pub fn max_level(&self) -> LevelFilter {
        self.0
            .split(',')
            .filter_map(|rule| parse_level(rule.split_once('=').map_or(rule, |(_, level)| level)))
            .max()
            .unwrap_or(LevelFilter::Info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all<const N: usize>(ring: &LogRing<N>, from: Option<u64>) -> (LogRead, Vec<u8>) {
        let mut output = [0; 64];
        let read = ring.read(from, &mut output);
        (read, output[..(read.next - read.start) as usize].to_vec())
    }

    #[test]
    fn reads_follow_a_cursor_with_nothing_missed() {
        let mut ring = LogRing::<16>::new();
        ring.push(b"abc");
        let (read, bytes) = read_all(&ring, Some(0));
        assert_eq!(read, LogRead { start: 0, next: 3 });
        assert_eq!(bytes, b"abc");
        ring.push(b"de");
        let (read, bytes) = read_all(&ring, Some(read.next));
        assert_eq!(read, LogRead { start: 3, next: 5 });
        assert_eq!(bytes, b"de");
        let (read, bytes) = read_all(&ring, Some(read.next));
        assert_eq!(read, LogRead { start: 5, next: 5 });
        assert!(bytes.is_empty());
    }

    #[test]
    fn wraparound_keeps_the_newest_bytes_in_order() {
        let mut ring = LogRing::<8>::new();
        ring.push(b"012345");
        ring.push(b"6789");
        let (read, bytes) = read_all(&ring, Some(2));
        assert_eq!(read, LogRead { start: 2, next: 10 });
        assert_eq!(bytes, b"23456789");
    }

    #[test]
    fn overwritten_offsets_start_at_the_oldest_byte() {
        let mut ring = LogRing::<8>::new();
        ring.push(b"0123456789");
        let (read, bytes) = read_all(&ring, Some(1));
        assert_eq!(read.start, 2);
        assert_eq!(bytes, b"23456789");
    }

    #[test]
    fn a_cursor_from_before_a_reboot_starts_over() {
        let mut ring = LogRing::<8>::new();
        ring.push(b"abc");
        let (read, bytes) = read_all(&ring, Some(1000));
        assert_eq!(read, LogRead { start: 0, next: 3 });
        assert_eq!(bytes, b"abc");
    }

    #[test]
    fn a_write_larger_than_the_ring_keeps_its_tail() {
        let mut ring = LogRing::<4>::new();
        ring.push(b"x");
        ring.push(b"0123456789");
        let (read, bytes) = read_all(&ring, None);
        assert_eq!(read, LogRead { start: 7, next: 11 });
        assert_eq!(bytes, b"6789");
    }

    #[test]
    fn without_a_cursor_the_newest_bytes_that_fit_are_read() {
        let mut ring = LogRing::<16>::new();
        ring.push(b"0123456789");
        let mut output = [0; 4];
        let read = ring.read(None, &mut output);
        assert_eq!(read, LogRead { start: 6, next: 10 });
        assert_eq!(&output, b"6789");
    }

    #[test]
    fn a_short_output_advances_the_cursor_only_as_far_as_copied() {
        let mut ring = LogRing::<16>::new();
        ring.push(b"0123456789");
        let mut output = [0; 4];
        let first = ring.read(Some(0), &mut output);
        assert_eq!(first, LogRead { start: 0, next: 4 });
        let second = ring.read(Some(first.next), &mut output);
        assert_eq!(second, LogRead { start: 4, next: 8 });
        assert_eq!(&output, b"4567");
    }

    #[test]
    fn filter_uses_the_most_specific_rule() {
        let filter = LogFilter("info,embassy_net=warn,embassy_net::tcp=debug");
        assert!(filter.enabled(Level::Info, "esp32_led_mqtt"));
        assert!(!filter.enabled(Level::Debug, "esp32_led_mqtt"));
        assert!(!filter.enabled(Level::Info, "embassy_net"));
        assert!(!filter.enabled(Level::Info, "embassy_net::dns"));
        assert!(filter.enabled(Level::Warn, "embassy_net::dns"));
        assert!(filter.enabled(Level::Debug, "embassy_net::tcp"));
        // A name that merely starts the same is a different crate.
        assert!(filter.enabled(Level::Info, "embassy_network"));
        assert_eq!(filter.max_level(), LevelFilter::Debug);
    }

    #[test]
    fn filter_defaults_to_info_and_ignores_rubbish() {
        for rules in ["", "nonsense", "x=nonsense"] {
            let filter = LogFilter(rules);
            assert!(filter.enabled(Level::Info, "any"), "{rules}");
            assert!(!filter.enabled(Level::Debug, "any"), "{rules}");
            assert_eq!(filter.max_level(), LevelFilter::Info, "{rules}");
        }
        assert!(!LogFilter("off").enabled(Level::Error, "any"));
        assert!(LogFilter("trace").enabled(Level::Trace, "any"));
    }
}
