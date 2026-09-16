//! `Record` for primitives, ids and blobs.

use super::Record;
use crate::id::Id;

macro_rules! impl_record_for_int {
    ($($t:ty),*) => {$(
        impl Record for $t {
            const SIZE: usize = size_of::<$t>();

            fn encode(&self, out: &mut [u8]) {
                out.copy_from_slice(&self.to_le_bytes());
            }

            fn decode(src: &[u8]) -> Self {
                let mut buf = [0u8; size_of::<$t>()];
                buf.copy_from_slice(src);
                <$t>::from_le_bytes(buf)
            }
        }
    )*};
}

impl_record_for_int!(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64);

impl Record for bool {
    const SIZE: usize = 1;

    fn encode(&self, out: &mut [u8]) {
        out[0] = *self as u8;
    }

    fn decode(src: &[u8]) -> Self {
        src[0] != 0
    }
}

impl<T: ?Sized + 'static> Record for Id<T> {
    const SIZE: usize = 8;

    fn encode(&self, out: &mut [u8]) {
        out.copy_from_slice(&self.raw().to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(src);
        Id::from_raw(u64::from_le_bytes(buf))
    }
}

impl<T: ?Sized + 'static> Record for Option<Id<T>> {
    const SIZE: usize = 8;

    fn encode(&self, out: &mut [u8]) {
        let raw = match self {
            Some(id) => {
                debug_assert!(
                    id.raw() != Id::<T>::NULL,
                    "Id::NULL cannot round-trip as Some"
                );
                id.raw()
            }
            None => Id::<T>::NULL,
        };
        out.copy_from_slice(&raw.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(src);
        let raw = u64::from_le_bytes(buf);
        if raw == Id::<T>::NULL {
            None
        } else {
            Some(Id::from_raw(raw))
        }
    }
}

#[cfg(test)]
#[path = "impls_test.rs"]
mod impls_test;
