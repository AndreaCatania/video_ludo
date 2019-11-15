use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use ffmpeg_sys as ffmpeg;
use rayon;
use ringbuf::{Producer, RingBuffer};

use crate::{
    daemon_reader::{DaemonCommand, DaemonReader},
    stream_reader::{stream_reader_new, StreamEntry, StreamInfo, StreamReader},
    Movie,
};

static FFMPEG_INIT: std::sync::Once = std::sync::Once::new();

/// The `MovieReader` is the object that allows you to read a Movie File,
/// it is able to asynchronously load and convert the video and audio data to
/// something that is understandable by your application.
///
/// The file reading is done in streaming; The data are processed and allocated
/// in a buffer of fixed size. The read information are deallocated from the buffer,
/// allowing for new data to being read.
///
/// Due to the asynchronous nature of this crate, some commands doesn't have
/// immediate effect (see: `play`, `stop`, `seek`).
///
/// *Check the __README.md__ to know more about it.*
///
/// # How to initialize it
/// To construct an istance of this class, you have to call the function `try_new`
/// that will return the `MovieReader` object and a list of `StreamReaderEntry`.
///
/// This function accept a list of `StreamInfo` that describes the internal
/// `StreamReader` that the `MovieReader` will create. Each `StreamReader` will
/// read a stream, and you can extract the read information using the
/// `StreamReaderEntry` returned.
///
/// *Check the __README.md__ to know more about it.*
#[allow(missing_debug_implementations)]
pub struct MovieReader {
    // The `movie` is never used by the `MovieReader` directly, but the task to read the movie
    // is assigned to the `DaemonReader`.
    // The `DaemonReader`, doesn't own the `Movie`; indeed, when it finish its task it get killed
    // and if required a new one is spawned.
    movie: Arc<Mutex<Movie>>,

    // The `stream_readers` is never used by the `MovieReader` directly, but the task to read the movie
    // is assigned to the `DaemonReader`.
    // The `DaemonReader`, doesn't own the `StreamReader`s; indeed, when it finish its task it get killed
    // and if required a new one is spawned.
    stream_readers: Arc<Mutex<Vec<Box<dyn StreamReader>>>>,

    // The communication to the `DaemonReader` is asynchronous and is implemented through a buffer
    // that is filled by the `MovieReader` and consumed by the `DaemonReader`.
    daemon_commands: Option<Producer<DaemonCommand>>,

    /// Define the sleep duration in ms.
    ///
    /// The `DaemonThread` go to sleep if a buffer is full before to retry read the packer, to relax
    /// the execution.
    ///
    /// By default it's set to 10ms, but it's better set this depending on the video frame rate;
    /// where something like 4 or 10 times the frame rate would be far enough:
    /// ```ignore
    /// 1000 / video_frame_rate * 4
    /// ```
    pub sleep_duration_ms: u8,
}

impl MovieReader {
    /// Try to create a new `MovieReader` this function may fail for different reasons:
    /// - The movie file doesn't exist.
    /// - The file is not a valid video.
    /// - There aren't codec available for this Movie.
    /// - The `buffer_size_mb` is not enough.
    /// - ...
    ///
    /// It's guaranteed that when succeed all the `StreamReader`s get created and a list of pointers
    /// to each stream entry is returned; is up to you convert these `Any` pointer to the correct class.
    ///
    /// - The video stream returns as entry the `Video` class
    // The reason why it's not returned an enum that converts these in a more natural way, is because
    // by doing so is possible to add `StreamReader`s that are not supported by this crate.
    pub fn try_new(
        movie_path: &Path,
        streams_info: Vec<StreamInfo>,
    ) -> Result<(Self, Vec<StreamEntry>), String> {
        if streams_info.is_empty() {
            return Err("The stream reader info is empty".to_owned());
        }

        FFMPEG_INIT.call_once(|| {
            unsafe {
                // Register all the FFmpeg codecs TODO please do it elsewhere and add a way to register only part of the codecs.
                ffmpeg::av_register_all();
                // TODO use logger?
                println!("All codec registered!");
            }
        });

        let mut movie = Movie::try_new(movie_path)?;

        // Let's create the `StreamReader` vector.
        let mut stream_readers = Vec::with_capacity(streams_info.len());

        // List of stream entries per each `StreamReader`.
        let mut stream_entries = Vec::with_capacity(streams_info.len());

        // Creates the `StreamReader`s
        for sri in streams_info.iter() {
            let (sr, sbc) = stream_reader_new(&mut movie, sri)?;
            stream_readers.push(sr);
            stream_entries.push(sbc);
        }

        Ok((
            MovieReader {
                movie: Arc::new(Mutex::new(movie)),
                stream_readers: Arc::new(Mutex::new(stream_readers)),
                daemon_commands: None,
                sleep_duration_ms: 10,
            },
            stream_entries,
        ))
    }

    /// Start reading the file, you can pause it by calling `stop_read`.
    /// This function doesn't never seek the movie timestamp so call it after `stop_read` will
    /// restart the reading where it left.
    ///
    /// When the file arrive to the end, if you want to re-read the file you have to call `seek(0.0)`.
    ///
    /// This command doesn't have an immediate effect
    pub fn start_read(&mut self) {
        if self.is_reading() {
            // Nothing to do
            return;
        }
        self.start_daemon();
    }

    /// Seek the actual reading to the timestamp in sec.
    ///
    /// If the reading is not yet started calling this function start it.
    ///
    /// If the reading is in progress is up to the user clear the stream buffers, using the provided
    /// function: `entry.buffer().clear();`
    ///
    /// This command doesn't have an immediate effect
    pub fn seek(&mut self, timestamp: f64) {
        self.start_read();
        let dc = self.daemon_commands.as_mut().unwrap();
        dc.push(DaemonCommand::Seek(timestamp))
            .expect("unreachable");
    }

    /// Stop reading the file, you can resume it by calling `start_read`.
    ///
    /// This command doesn't have an immediate effect
    pub fn stop_read(&mut self) {
        if let Some(dc) = &mut self.daemon_commands {
            dc.push(DaemonCommand::Stop).expect("unreachable");
        }
    }

    /// Return true only when the reading is really started and false otherwise.
    ///
    /// It's not guaranteed that this function returns true just after `start_read` is called.
    pub fn is_reading(&self) -> bool {
        self.exist_daemon()
    }

    fn exist_daemon(&self) -> bool {
        Arc::strong_count(&self.movie) + Arc::weak_count(&self.movie) != 1
    }

    fn start_daemon(&mut self) {
        if self.exist_daemon() {
            panic!("You can't start a new daemon if another one is already running.");
        }

        if Arc::strong_count(&self.stream_readers) + Arc::weak_count(&self.stream_readers) != 1 {
            unreachable!();
        }

        if self.daemon_commands.is_some() {
            unreachable!();
        }

        let (producer, consumer) = RingBuffer::new(20).split();

        self.daemon_commands = Some(producer);

        let daemon = DaemonReader {
            movie: self.movie.clone(),
            stream_readers: self.stream_readers.clone(),
            commands: consumer,
            sleep_duration_ms: self.sleep_duration_ms,
        };

        rayon::spawn(move || {
            DaemonReader::process(daemon);
        });
    }
}

impl Drop for MovieReader {
    fn drop(&mut self) {
        self.stop_read();
    }
}

unsafe impl std::marker::Send for MovieReader {}
