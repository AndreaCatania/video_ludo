#[macro_use]
extern crate glium;

use std::path::Path;

use glium::{
    texture::{
        ClientFormat, RawImage2d,
    },
    Texture2d
};

use video_ludo::{
    stream_reader::{StreamReaderInfo, video_reader::Video},
    MovieReader
};

// This example just show how to use a `MovieReader`, with default `StreamReaderInfo`.
fn main() {
    // Stream reader list that the `MovieReader` will internally create.
    let mut stream_readers = Vec::new();

    // Describe a `StreamReader` that take the best video stream.
    stream_readers.push(StreamReaderInfo::best_video());

    // Creates the `MovieReader`, this function returns a `MovieReader` and a
    // list containing the `StreamReaderEntries` which can be used to extract
    // the stream data.
    let (mut movie_reader, mut stream_entries) = MovieReader::try_new(Path::new("resources/Ettore.ogv"), stream_readers)
        .expect("Movie Reader creation fail!");

    // Takes the stream buffer entry
    let mut video = stream_entries.pop().unwrap().downcast::<Video>().unwrap();

    // Start reading the file.
    movie_reader.start_read();

    
    utils::render(|display| -> Option<Texture2d> {
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
        }else{
            None
        }
    });
}

mod utils;