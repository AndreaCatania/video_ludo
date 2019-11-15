#[macro_use]
extern crate glium;

use std::path::Path;

use glium::{
    texture::{ClientFormat, RawImage2d},
    Texture2d,
};

use video_ludo::{
    stream_reader::{video_reader::Video, StreamInfo},
    MovieReader,
};

// This example just show how to use a `MovieReader`, with default `StreamReaderInfo`,
// to extract frames from a stream.
//
// _Disclaimer_
// You may notice that the playback speed of the video is too fast.
// This happens, because the video frames are not sync to a timer; indeed, is not
// intention of this crate implement a Movie **Player**, rather leaves to you
// the freedom to do it (or not).
fn main() {
    // Stream reader list that the `MovieReader` will internally create.
    let mut stream_readers = Vec::new();

    // Use a the best video stream
    stream_readers.push(StreamInfo::best_video());

    // Creates the `MovieReader`, this function returns a `MovieReader` and a
    // list containing the `StreamReaderEntries` which can be used to extract
    // the stream data.
    let (mut movie_reader, mut stream_entries) =
        MovieReader::try_new(Path::new("resources/Ettore.ogv"), stream_readers)
            .expect("Movie Reader creation fail!");

    // Takes the stream buffer entry
    let mut video = stream_entries.pop().unwrap().downcast::<Video>().unwrap();

    // Start reading the file.
    movie_reader.start_read();

    // Start the rendering
    utils::render(|display| -> Option<Texture2d> {
        // Each frame this code is executed.

        // Take a frame from the video buffer.
        if let Some(frame) = video.buffer_mut().pop() {
            // Copy the data into a texture.
            let raw = RawImage2d {
                data: std::borrow::Cow::Owned(frame.data.clone()),
                width: video.frame_size()[0],
                height: video.frame_size()[1],
                format: ClientFormat::U8U8U8,
            };

            // Just free the frame data.
            video.buffer_mut().finalize_pop();

            Some(Texture2d::new(display, raw).expect("Texture not stored!"))
        } else {
            None
        }
    });
}

mod utils;
