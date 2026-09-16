//! Fixed-width little-endian encoding for column elements.

mod impls;

pub trait Record: Copy + 'static {
    const SIZE: usize;

    fn encode(&self, out: &mut [u8]);

    fn decode(src: &[u8]) -> Self;
}
