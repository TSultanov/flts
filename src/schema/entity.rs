//! What every entity knows about itself: the name its columns are qualified by.

pub trait Entity: 'static {
    const NAME: &'static str;
}
