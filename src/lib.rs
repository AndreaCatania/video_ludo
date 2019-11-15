//! The Video Ludo crate is a movie reader, written in rust lang, which allows
//! to extract information from the various streams that compose a movie file.
//! 
//! To start using it you have to create an istance of 'MovieReader`, check the
//! example or the __README.md__ to get more info about it.

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
