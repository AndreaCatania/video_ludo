#![warn(
    missing_debug_implementations,
    rust_2018_idioms,
    rust_2018_compatibility,
    clippy::all
)]

pub use movie::Movie;
pub use movie_reader::MovieReader;

mod daemon_reader;
mod movie;
mod movie_reader;
pub mod stream_reader;
