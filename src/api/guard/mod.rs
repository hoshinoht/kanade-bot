//! Request guards shared by both listeners, applied in `listeners::router`.

pub mod headers;
pub mod host;
pub mod limits;
pub mod proxy;
