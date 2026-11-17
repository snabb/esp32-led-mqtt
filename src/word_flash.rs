//! Byte-level `embedded-storage` access over a flash driver that reads and programs whole
//! 32-bit words, as esp-hal's `Flash` does.
//!
//! `ReadStorage`/`Storage` take any offset and length; `Storage::write` erases and rewrites
//! each touched sector, so it doesn't need erased flash. `ReadNorFlash`/`NorFlash` require
//! word-aligned access and erased flash for writes, as their contract says.

use embedded_storage::{
    ReadStorage, Storage,
    nor_flash::{
        ErrorType, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash, check_erase,
        check_read, check_write,
    },
};

pub const WORD_SIZE: u32 = 4;
pub const SECTOR_SIZE: u32 = 4096;
const SECTOR_WORDS: usize = (SECTOR_SIZE / WORD_SIZE) as usize;
/// Words staged per driver call outside sector rewrites, to keep stack use small.
const CHUNK_WORDS: usize = 64;

/// A flash driver working in whole words. Word bytes are in flash order, so
/// `u32::to_le_bytes` gives the bytes as stored.
pub trait WordFlash {
    type Error: core::fmt::Debug;

    fn capacity(&self) -> u32;
    /// `offset` is word-aligned and the range within `capacity`.
    fn read_words(&mut self, offset: u32, words: &mut [u32]) -> Result<(), Self::Error>;
    /// Programs erased flash. `offset` is word-aligned and the range within `capacity`.
    fn write_words(&mut self, offset: u32, words: &[u32]) -> Result<(), Self::Error>;
    /// Erases `[from, to)`; both are sector-aligned.
    fn erase_sectors(&mut self, from: u32, to: u32) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    OutOfBounds,
    NotAligned,
    Flash(E),
}

impl<E: core::fmt::Debug> NorFlashError for Error<E> {
    fn kind(&self) -> NorFlashErrorKind {
        match self {
            Self::OutOfBounds => NorFlashErrorKind::OutOfBounds,
            Self::NotAligned => NorFlashErrorKind::NotAligned,
            Self::Flash(_) => NorFlashErrorKind::Other,
        }
    }
}

impl<E> From<NorFlashErrorKind> for Error<E> {
    fn from(kind: NorFlashErrorKind) -> Self {
        match kind {
            NorFlashErrorKind::NotAligned => Self::NotAligned,
            _ => Self::OutOfBounds,
        }
    }
}

pub struct ByteFlash<F>(pub F);

impl<F: WordFlash> ByteFlash<F> {
    fn check_bounds(&self, offset: u32, len: usize) -> Result<(), Error<F::Error>> {
        let end = u32::try_from(len)
            .ok()
            .and_then(|len| offset.checked_add(len))
            .ok_or(Error::OutOfBounds)?;
        if end > self.0.capacity() {
            return Err(Error::OutOfBounds);
        }
        Ok(())
    }

    /// Reads any range, staging whole words.
    pub fn read_bytes(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Error<F::Error>> {
        self.check_bounds(offset, bytes.len())?;
        let mut words = [0u32; CHUNK_WORDS];
        let mut position = offset;
        let mut bytes = bytes;
        while !bytes.is_empty() {
            let aligned = position - position % WORD_SIZE;
            let skip = (position - aligned) as usize;
            let len = bytes.len().min(CHUNK_WORDS * WORD_SIZE as usize - skip);
            // Rounding up stays within capacity, which is a whole number of words.
            let count = (skip + len).div_ceil(WORD_SIZE as usize);
            self.0
                .read_words(aligned, &mut words[..count])
                .map_err(Error::Flash)?;
            for (index, byte) in bytes[..len].iter_mut().enumerate() {
                *byte = byte_at(&words, skip + index);
            }
            position += len as u32;
            bytes = &mut bytes[len..];
        }
        Ok(())
    }

    /// Programs erased flash at a word-aligned offset with a whole number of words.
    fn write_aligned(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Error<F::Error>> {
        let mut words = [0u32; CHUNK_WORDS];
        for (index, chunk) in bytes.chunks(CHUNK_WORDS * WORD_SIZE as usize).enumerate() {
            let count = chunk.len() / WORD_SIZE as usize;
            for (word, source) in words.iter_mut().zip(chunk.as_chunks::<4>().0) {
                *word = u32::from_le_bytes(*source);
            }
            let at = offset + (index * CHUNK_WORDS) as u32 * WORD_SIZE;
            self.0
                .write_words(at, &words[..count])
                .map_err(Error::Flash)?;
        }
        Ok(())
    }

    /// Writes any range by erasing and rewriting each touched sector.
    pub fn rewrite_bytes(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Error<F::Error>> {
        self.check_bounds(offset, bytes.len())?;
        let mut sector = [0u32; SECTOR_WORDS];
        let mut position = offset;
        let mut bytes = bytes;
        while !bytes.is_empty() {
            let start = position - position % SECTOR_SIZE;
            let skip = (position - start) as usize;
            let len = bytes.len().min(SECTOR_SIZE as usize - skip);
            self.0
                .read_words(start, &mut sector)
                .map_err(Error::Flash)?;
            for (index, &byte) in bytes[..len].iter().enumerate() {
                set_byte(&mut sector, skip + index, byte);
            }
            self.0
                .erase_sectors(start, start + SECTOR_SIZE)
                .map_err(Error::Flash)?;
            self.0.write_words(start, &sector).map_err(Error::Flash)?;
            position += len as u32;
            bytes = &bytes[len..];
        }
        Ok(())
    }
}

fn byte_at(words: &[u32], index: usize) -> u8 {
    words[index / 4].to_le_bytes()[index % 4]
}

fn set_byte(words: &mut [u32], index: usize, byte: u8) {
    let mut bytes = words[index / 4].to_le_bytes();
    bytes[index % 4] = byte;
    words[index / 4] = u32::from_le_bytes(bytes);
}

impl<F: WordFlash> ErrorType for ByteFlash<F> {
    type Error = Error<F::Error>;
}

impl<F: WordFlash> ReadNorFlash for ByteFlash<F> {
    const READ_SIZE: usize = WORD_SIZE as usize;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), <Self as ErrorType>::Error> {
        check_read(self, offset, bytes.len())?;
        self.read_bytes(offset, bytes)
    }

    fn capacity(&self) -> usize {
        self.0.capacity() as usize
    }
}

impl<F: WordFlash> NorFlash for ByteFlash<F> {
    const WRITE_SIZE: usize = WORD_SIZE as usize;
    const ERASE_SIZE: usize = SECTOR_SIZE as usize;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), <Self as ErrorType>::Error> {
        check_erase(self, from, to)?;
        self.0.erase_sectors(from, to).map_err(Error::Flash)
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), <Self as ErrorType>::Error> {
        check_write(self, offset, bytes.len())?;
        self.write_aligned(offset, bytes)
    }
}

impl<F: WordFlash> ReadStorage for ByteFlash<F> {
    type Error = Error<F::Error>;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.read_bytes(offset, bytes)
    }

    fn capacity(&self) -> usize {
        self.0.capacity() as usize
    }
}

impl<F: WordFlash> Storage for ByteFlash<F> {
    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        self.rewrite_bytes(offset, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPACITY: u32 = 4 * SECTOR_SIZE;

    /// Byte-addressed NOR flash: erase sets 0xFF, programming can only clear bits.
    struct MockFlash(Vec<u8>);

    impl MockFlash {
        fn new() -> Self {
            Self((0..CAPACITY).map(|i| i as u8).collect())
        }
    }

    impl WordFlash for MockFlash {
        type Error = ();

        fn capacity(&self) -> u32 {
            CAPACITY
        }

        fn read_words(&mut self, offset: u32, words: &mut [u32]) -> Result<(), ()> {
            assert_eq!(offset % WORD_SIZE, 0);
            for (index, word) in words.iter_mut().enumerate() {
                let at = offset as usize + index * 4;
                *word = u32::from_le_bytes(self.0[at..at + 4].try_into().unwrap());
            }
            Ok(())
        }

        fn write_words(&mut self, offset: u32, words: &[u32]) -> Result<(), ()> {
            assert_eq!(offset % WORD_SIZE, 0);
            for (index, word) in words.iter().enumerate() {
                let at = offset as usize + index * 4;
                for (stored, new) in self.0[at..at + 4].iter_mut().zip(word.to_le_bytes()) {
                    *stored &= new;
                }
            }
            Ok(())
        }

        fn erase_sectors(&mut self, from: u32, to: u32) -> Result<(), ()> {
            assert_eq!(from % SECTOR_SIZE, 0);
            assert_eq!(to % SECTOR_SIZE, 0);
            self.0[from as usize..to as usize].fill(0xFF);
            Ok(())
        }
    }

    #[test]
    fn reads_any_offset_and_length() {
        let mut flash = ByteFlash(MockFlash::new());
        for (offset, len) in [(0, 1), (1, 3), (3, 300), (4093, 7), (CAPACITY - 5, 5)] {
            let mut bytes = vec![0; len];
            flash.read_bytes(offset, &mut bytes).unwrap();
            let expected: Vec<u8> = (offset..offset + len as u32).map(|i| i as u8).collect();
            assert_eq!(bytes, expected, "offset {offset} len {len}");
        }
        assert_eq!(
            flash.read_bytes(CAPACITY - 1, &mut [0; 2]),
            Err(Error::OutOfBounds)
        );
    }

    #[test]
    fn rewrite_keeps_the_rest_of_each_sector() {
        let mut flash = ByteFlash(MockFlash::new());
        let data: Vec<u8> = (0..5000).map(|i| (i * 7) as u8).collect();
        let offset = SECTOR_SIZE - 3;
        Storage::write(&mut flash, offset, &data).unwrap();
        let mut all = vec![0; CAPACITY as usize];
        flash.read_bytes(0, &mut all).unwrap();
        for (at, &byte) in all.iter().enumerate() {
            let at = at as u32;
            let expected = if (offset..offset + data.len() as u32).contains(&at) {
                data[(at - offset) as usize]
            } else {
                at as u8
            };
            assert_eq!(byte, expected, "byte {at}");
        }
    }

    #[test]
    fn nor_flash_enforces_alignment_and_programs_erased_flash() {
        let mut flash = ByteFlash(MockFlash::new());
        assert_eq!(
            NorFlash::write(&mut flash, 2, &[0; 4]),
            Err(Error::NotAligned)
        );
        assert_eq!(NorFlash::erase(&mut flash, 0, 100), Err(Error::NotAligned));
        NorFlash::erase(&mut flash, SECTOR_SIZE, 2 * SECTOR_SIZE).unwrap();
        let data: Vec<u8> = (0..512).map(|i| i as u8 ^ 0x5a).collect();
        NorFlash::write(&mut flash, SECTOR_SIZE + 8, &data).unwrap();
        let mut back = vec![0; data.len()];
        ReadNorFlash::read(&mut flash, SECTOR_SIZE + 8, &mut back).unwrap();
        assert_eq!(back, data);
    }
}
