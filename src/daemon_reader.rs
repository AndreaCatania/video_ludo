use std::sync::{Arc, Mutex};

use ffmpeg_sys as ffmpeg;
use ringbuf::Consumer;

use crate::{
    stream_reader::{ReadingResult, StreamReader},
    Movie,
};

// The `DaemonReader` perform the actual reading. We can have only one active at a time.
pub(crate) struct DaemonReader {
    pub(crate) movie: Arc<Mutex<Movie>>,
    pub(crate) stream_readers: Arc<Mutex<Vec<Box<dyn StreamReader>>>>,
    /// Commands that can be written
    pub(crate) commands: Consumer<DaemonCommand>,
    pub(crate) sleep_duration_ms: u8,
}

impl DaemonReader {
    // The `DaemonReader` iterates all over the `Movie` packets, one at a time, and feed these to
    // the `StreamReader`s.
    //
    // A `StreamReader` may not be ready to collect the packet info at that particular moment in time,
    // (probably the buffer is full) and it notify the `DaemonReader` about it.
    // The `DaemonReader` does keep tracks of all these `StreamReader`s, and try to feed this packet
    // after a short amount of time.
    //
    // The `DaemonReader` doesn't never block the execution to wait that a `StreamReader` is
    // again available to read.
    pub(crate) fn process(mut daemon: DaemonReader) {
        let mut movie = daemon.movie.lock().expect("Unreachable");
        let mut stream_readers = daemon.stream_readers.lock().expect("Unreachable");
        let mut packet: ffmpeg::AVPacket = unsafe { std::mem::zeroed() };

        // List of `StreamReader`s that can't acquire the information from the `Packet` right now.
        let mut delayed_list = Vec::<Option<usize>>::with_capacity(stream_readers.len());

        loop {
            // Command execution
            let mut stop_reading = false;
            daemon.commands.pop_each(
                |cmd| -> bool {
                    match cmd {
                        DaemonCommand::Stop => {
                            stop_reading = true;
                            false /* Stop the `pop_each` here */
                        }
                        DaemonCommand::Seek(timestamp) => {
                            let mut stream_id = None;
                            let mut time_base = 0.0;
                            for sr in stream_readers.iter_mut() {
                                if stream_id.is_none() {
                                    stream_id = Some(sr.stream_id());
                                    time_base = sr.stream_time_base();
                                }
                                sr.clear_internal_buffer();
                            }

                            unsafe {
                                if ffmpeg::av_seek_frame(
                                    movie.context_ptr(),
                                    stream_id.unwrap() as i32,
                                    (timestamp / time_base) as i64,
                                    ffmpeg::AVSEEK_FLAG_ANY,
                                ) < 0
                                {
                                    // TODO use a logger?
                                    println!(
                                        "Was not possible to perform the seek, timestamp: {}",
                                        timestamp
                                    );
                                    stop_reading = true;
                                    false /* Stop the `pop_each` here */
                                } else {
                                    true /* Continue `pop_each` */
                                }
                            }
                        }
                    }
                },
                None,
            );

            if stop_reading {
                // Stop the reading process
                break;
            }

            // Packet process
            if delayed_list.is_empty() {
                // The delayed list is void so a new packet can be processed
                if unsafe { ffmpeg::av_read_frame(movie.context_ptr(), &mut packet) } == 0 {
                    // Feed this packet to each `StreamReader`
                    let r = DaemonReader::process_packet_first_pass(
                        &packet,
                        &mut stream_readers,
                        &mut delayed_list,
                    );

                    if let Err(msg) = r {
                        // TODO use a logger?
                        println!("Daemon error: {}", msg);
                        // Error stop the daemon.
                        unsafe {
                            ffmpeg::av_packet_unref(&mut packet);
                        }
                        break;
                    }
                } else {
                    // All the packets have been read, stop the daemon.
                    break;
                }
            } else {
                // Some `StreamReader`s have to read this packet yet.
                let r = DaemonReader::process_packet_delayed(
                    &packet,
                    &mut stream_readers,
                    &mut delayed_list,
                    daemon.sleep_duration_ms,
                );
                if let Err(msg) = r {
                    // TODO use a logger?
                    println!("Daemon error: {}", msg);
                    // Error stop the daemon.
                    unsafe {
                        ffmpeg::av_packet_unref(&mut packet);
                    }
                    break;
                }
            }

            if delayed_list.is_empty() {
                unsafe {
                    // At this point for sure the `packet` is read by all the SR.
                    // So free it.
                    ffmpeg::av_packet_unref(&mut packet);
                }
            }
        }

        for sr in stream_readers.iter_mut() {
            sr.finalize();
        }
    }

    // Process a new read packet, this is executed once per packet
    fn process_packet_first_pass(
        packet: &ffmpeg::AVPacket,
        stream_readers: &mut Vec<Box<dyn StreamReader>>,
        delayed_list: &mut Vec<Option<usize>>,
    ) -> Result<(), String> {
        for (stream_id, sr) in stream_readers.iter_mut().enumerate() {
            let res = sr.read_packet(&packet)?;
            if let ReadingResult::BufferFull = res {
                // The packet can't be read now, insert in the delayed list
                delayed_list.push(Some(stream_id));
            }
        }
        Ok(())
    }

    // Process the packet until all the `StreamReader`s have finish with it.
    //
    // When a `StreamReader` reader finally the packet, it is removed from the delayed list;
    // when the delayed list is empty this process is finished.
    //
    // When in a particular moment any `StreamReader` read the packet, the function may put the
    // thread in sleep to relax the execution.
    fn process_packet_delayed(
        packet: &ffmpeg::AVPacket,
        stream_readers: &mut Vec<Box<dyn StreamReader>>,
        delayed_list: &mut Vec<Option<usize>>,
        sleep_duration_ms: u8,
    ) -> Result<(), String> {
        // These two variables are used by the mechanism that sets `want_to_wait` to `false`
        // when a `StreamReader` does something just after another `StreamReader` want
        // to wait.
        // Indeed, in this case we don't need to wait, because the SR execution already
        // taken some time.
        let mut want_to_wait = true;
        let mut someone_need_to_wait = false;

        for stream_id in delayed_list.iter_mut() {
            let sr = &mut stream_readers[stream_id.unwrap()];
            let res = sr.read_packet(&packet)?;
            match res {
                ReadingResult::Done => {
                    // Finally done!
                    *stream_id = None;
                    if someone_need_to_wait {
                        want_to_wait = false;
                    }
                }
                ReadingResult::BufferFull => {
                    // Not yet ready, continue waiting
                    someone_need_to_wait = true;
                }
            }
        }

        delayed_list.retain(|v| -> bool { v.is_some() });

        if !delayed_list.is_empty() {
            // Delay a bit the execution before retry if nothing happen in this cycle.
            if want_to_wait {
                let time = std::time::Duration::from_millis(sleep_duration_ms.into());
                std::thread::sleep(time);
            }
        }

        Ok(())
    }
}

unsafe impl std::marker::Send for DaemonReader {}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DaemonCommand {
    Stop,
    Seek(f64),
}
