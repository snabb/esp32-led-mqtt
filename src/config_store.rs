//! One versioned configuration snapshot in sequential-storage's CRC-protected map.

use crate::device_config::{DeviceConfig, Error as ConfigError, MAX_BYTES};
use core::ops::Range;
use embedded_storage_async::nor_flash::NorFlash;
use sequential_storage::{
    cache::Cache,
    map::{MapConfig, MapStorage},
};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Layout,
    Storage,
    Invalid(ConfigError),
    GenerationExhausted,
    Verification,
}

#[repr(align(4))]
struct Buffer([u8; MAX_BYTES]);

fn check<F: NorFlash>(flash: &F, range: &Range<u32>) -> Result<(), Error> {
    if range.start >= range.end || range.end as usize > flash.capacity() {
        return Err(Error::Layout);
    }
    MapConfig::<&mut F>::try_new(range.clone()).map_err(|_| Error::Layout)?;
    Ok(())
}

/// Load without replacing corrupt or unsupported settings with defaults.
pub async fn load<F: NorFlash>(
    flash: &mut F,
    range: Range<u32>,
) -> Result<Option<DeviceConfig>, Error> {
    check(flash, &range)?;
    let mut map = MapStorage::<(), _, _>::new(flash, MapConfig::new(range), Cache::new_uncached());
    let mut buffer = Buffer([0; MAX_BYTES]);
    let value = map
        .fetch_item::<&[u8]>(&mut buffer.0, &())
        .await
        .map_err(|_| Error::Storage)?;
    value
        .map(DeviceConfig::decode)
        .transpose()
        .map_err(Error::Invalid)
}

/// Save the entire snapshot atomically at the map level, then verify it by reading it back.
/// A failed/cancelled write may have committed; callers must reload rather than assume either outcome.
pub async fn save<F: NorFlash>(
    flash: &mut F,
    range: Range<u32>,
    mut config: DeviceConfig,
) -> Result<u32, Error> {
    config.validate().map_err(Error::Invalid)?;
    let old = load(flash, range.clone()).await?;
    config.generation = old
        .map_or(0, |old| old.generation)
        .checked_add(1)
        .ok_or(Error::GenerationExhausted)?;
    let mut encoded = Buffer([0; MAX_BYTES]);
    let len = config.encode(&mut encoded.0).map_err(Error::Invalid)?;
    {
        let mut map = MapStorage::<(), _, _>::new(
            &mut *flash,
            MapConfig::new(range.clone()),
            Cache::new_uncached(),
        );
        let mut scratch = Buffer([0; MAX_BYTES]);
        map.store_item(&mut scratch.0, &(), &&encoded.0[..len])
            .await
            .map_err(|_| Error::Storage)?;
    }
    let verified = load(flash, range).await?.ok_or(Error::Verification)?;
    let mut scratch = Buffer([0; MAX_BYTES]);
    let verified_len = verified.encode(&mut scratch.0).map_err(Error::Invalid)?;
    if verified_len != len || scratch.0[..len] != encoded.0[..len] {
        return Err(Error::Verification);
    }
    Ok(config.generation)
}

/// Explicit local reset only; never called as automatic corruption recovery.
pub async fn erase<F: NorFlash>(flash: &mut F, range: Range<u32>) -> Result<(), Error> {
    check(flash, &range)?;
    for from in (range.start..range.end).step_by(F::ERASE_SIZE) {
        flash
            .erase(from, from + F::ERASE_SIZE as u32)
            .await
            .map_err(|_| Error::Storage)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_storage_async::nor_flash::{
        ErrorType, NorFlashError, NorFlashErrorKind, ReadNorFlash,
    };
    use futures::executor::block_on;

    #[derive(Clone)]
    struct Flash {
        bytes: Vec<u8>,
        budget: Option<usize>,
    }
    #[derive(Debug, Clone, Copy)]
    struct PowerLoss;
    impl NorFlashError for PowerLoss {
        fn kind(&self) -> NorFlashErrorKind {
            NorFlashErrorKind::Other
        }
    }
    impl ErrorType for Flash {
        type Error = PowerLoss;
    }
    impl Flash {
        fn new() -> Self {
            Self {
                bytes: vec![0xff; 8192],
                budget: None,
            }
        }
        fn consume(&mut self) -> Result<(), PowerLoss> {
            if let Some(left) = self.budget.as_mut() {
                if *left == 0 {
                    return Err(PowerLoss);
                }
                *left -= 1;
            }
            Ok(())
        }
    }
    impl ReadNorFlash for Flash {
        const READ_SIZE: usize = 4;
        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), PowerLoss> {
            assert!(offset.is_multiple_of(4) && bytes.len().is_multiple_of(4));
            bytes.copy_from_slice(&self.bytes[offset as usize..offset as usize + bytes.len()]);
            Ok(())
        }
        fn capacity(&self) -> usize {
            self.bytes.len()
        }
    }
    impl NorFlash for Flash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = 4096;
        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), PowerLoss> {
            assert!(offset.is_multiple_of(4) && bytes.len().is_multiple_of(4));
            for (i, &byte) in bytes.iter().enumerate() {
                self.consume()?;
                let stored = &mut self.bytes[offset as usize + i];
                assert_eq!(*stored & byte, byte, "NOR write attempted 0-to-1");
                *stored &= byte;
            }
            Ok(())
        }
        async fn erase(&mut self, from: u32, to: u32) -> Result<(), PowerLoss> {
            assert!(from.is_multiple_of(4096) && to.is_multiple_of(4096));
            for i in from as usize..to as usize {
                self.consume()?;
                self.bytes[i] = 0xff;
            }
            Ok(())
        }
    }
    fn config() -> DeviceConfig {
        DeviceConfig::decode(br#"{"wifi_ssid":"network","wifi_password":"password1","mqtt_broker":"broker.example"}"#).unwrap()
    }

    #[test]
    fn snapshot_generation_and_explicit_erase() {
        block_on(async {
            let mut flash = Flash::new();
            assert!(load(&mut flash, 0..8192).await.unwrap().is_none());
            assert_eq!(save(&mut flash, 0..8192, config()).await, Ok(1));
            assert_eq!(save(&mut flash, 0..8192, config()).await, Ok(2));
            assert_eq!(
                load(&mut flash, 0..8192).await.unwrap().unwrap().generation,
                2
            );
            erase(&mut flash, 0..8192).await.unwrap();
            assert!(load(&mut flash, 0..8192).await.unwrap().is_none());
            assert_eq!(load(&mut flash, 1..8192).await.err(), Some(Error::Layout));
        });
    }

    #[test]
    fn unsupported_schema_is_preserved_until_an_explicit_reset() {
        block_on(async {
            let mut flash = Flash::new();
            let future = br#"{"schema":2,"wifi_ssid":"network","wifi_password":"password1","mqtt_broker":"broker.example"}"#;
            {
                let mut map = MapStorage::<(), _, _>::new(
                    &mut flash,
                    MapConfig::new(0..8192),
                    Cache::new_uncached(),
                );
                map.store_item(&mut [0; MAX_BYTES], &(), &future.as_slice())
                    .await
                    .unwrap();
            }
            let before = flash.bytes.clone();
            assert_eq!(
                load(&mut flash, 0..8192).await.err(),
                Some(Error::Invalid(ConfigError::Schema))
            );
            assert_eq!(
                save(&mut flash, 0..8192, config()).await,
                Err(Error::Invalid(ConfigError::Schema))
            );
            assert_eq!(flash.bytes, before);
        });
    }

    #[test]
    fn interrupted_replacement_keeps_an_entire_old_or_new_snapshot() {
        block_on(async {
            let mut initial = Flash::new();
            save(&mut initial, 0..8192, config()).await.unwrap();
            for budget in 0..1024 {
                let mut flash = initial.clone();
                flash.budget = Some(budget);
                let mut next = config();
                next.wifi_ssid = "replacement".try_into().unwrap();
                let _ = save(&mut flash, 0..8192, next).await;
                flash.budget = None;
                let result = load(&mut flash, 0..8192).await.unwrap().unwrap();
                assert!(
                    matches!(
                        (result.generation, result.wifi_ssid.as_str()),
                        (1, "network") | (2, "replacement")
                    ),
                    "torn snapshot after budget {budget}"
                );
            }
        });
    }
}
