//! `Record` for primitives, ids and blobs.

use super::Record;
use crate::id::{Id, Uuid};

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

impl Record for u128 {
    const SIZE: usize = size_of::<u128>();

    fn encode(&self, out: &mut [u8]) {
        out.copy_from_slice(&self.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        let mut buf = [0u8; size_of::<u128>()];
        buf.copy_from_slice(src);
        u128::from_le_bytes(buf)
    }
}

impl Record for () {
    const SIZE: usize = 0;

    fn encode(&self, _out: &mut [u8]) {}

    fn decode(_src: &[u8]) -> Self {}
}

impl Record for bool {
    const SIZE: usize = size_of::<u8>();

    fn encode(&self, out: &mut [u8]) {
        out[0] = *self as u8;
    }

    fn decode(src: &[u8]) -> Self {
        src[0] != 0
    }
}

impl<T: ?Sized + 'static> Record for Id<T> {
    const SIZE: usize = size_of::<u128>();

    fn encode(&self, out: &mut [u8]) {
        out.copy_from_slice(&self.uuid().to_u128().to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        let mut buf = [0u8; 16];
        buf.copy_from_slice(src);
        Id::from_uuid(Uuid::from_u128(u128::from_le_bytes(buf)))
    }
}

impl<T: ?Sized + 'static> Record for Option<Id<T>> {
    const SIZE: usize = size_of::<u128>();

    fn encode(&self, out: &mut [u8]) {
        let raw = match self {
            Some(id) => {
                debug_assert!(!id.is_nil(), "a nil Id cannot round-trip as Some");
                id.uuid().to_u128()
            }
            None => Uuid::NIL.to_u128(),
        };
        out.copy_from_slice(&raw.to_le_bytes());
    }

    fn decode(src: &[u8]) -> Self {
        let mut buf = [0u8; 16];
        buf.copy_from_slice(src);
        let id = Id::from_uuid(Uuid::from_u128(u128::from_le_bytes(buf)));
        (!id.is_nil()).then_some(id)
    }
}

#[cfg(test)]
#[path = "impls_test.rs"]
mod impls_test;
