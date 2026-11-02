//! C runtime entry points not exported by the C6 ROM bindings, needed by MbedTLS.

/// # Safety
/// `ptr` must be readable for `len` bytes, unless `len` is zero.
#[cfg_attr(target_arch = "riscv32", unsafe(no_mangle))]
unsafe extern "C" fn memchr(ptr: *const u8, value: i32, len: usize) -> *mut u8 {
    for i in 0..len {
        // SAFETY: the caller supplies the readable range.
        let at = unsafe { ptr.add(i) };
        if unsafe { at.read() } == value as u8 {
            return at.cast_mut();
        }
    }
    core::ptr::null_mut()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memchr_obeys_c_byte_and_length_semantics() {
        let bytes = [0, 255, 42, 42];
        // SAFETY: the array covers every nonempty search range.
        unsafe {
            assert_eq!(
                memchr(bytes.as_ptr(), -1, 4),
                bytes.as_ptr().add(1).cast_mut()
            );
            assert_eq!(
                memchr(bytes.as_ptr(), 42, 4),
                bytes.as_ptr().add(2).cast_mut()
            );
            assert!(memchr(bytes.as_ptr(), 42, 2).is_null());
            assert!(memchr(core::ptr::null(), 0, 0).is_null());
        }
    }
}
