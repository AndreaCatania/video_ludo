use std::collections::VecDeque;

use ffmpeg_sys as ffmpeg;
use ringbuf::{Consumer, Producer, RingBuffer};

use crate::{
    stream_reader::video_reader::{OutImageSize, OutPixelFormat, VideoStreamReader},
    Movie,
};

/// `StreamEntry` is a pointer that casted to its proper type allow to retrieve
/// the `StreamReader` information.
///
/// - The `VideoStreamReader` return a pointer to a `video_ludo::stream_reader::video_reader::Video` object.
// TODO please add the Audio and subtitles here once supported
pub type StreamEntry = Box<dyn std::any::Any>;

/// This is an utility function that creates the correct `StreamReader` depending on the stream info.
pub(crate) fn stream_reader_new(
    movie: &mut Movie,
    stream_info: &StreamInfo,
) -> Result<(Box<dyn StreamReader>, StreamEntry), String> {
    match stream_info {
        StreamInfo::Video {
            filter_frame_width,
            filter_frame_height,
            out_image_type,
            out_image_size,
            buffer_size_mb,
        } => {
            let (vsr, bc) = VideoStreamReader::try_new(
                movie,
                *filter_frame_width,
                *filter_frame_height,
                *out_image_type,
                *out_image_size,
                *buffer_size_mb,
            )?;

            Ok((Box::new(vsr), Box::new(bc)))
        }
    }
}

/// This enum is used to create a `StreamReader`, it has some filters that are used to select the correct
/// stream from the Movie file.
#[derive(Debug)]
pub enum StreamInfo {
    /// With this type is create a `StreamReader` that reads from a stream an convert the video image
    /// to the specified `out_image_type` type.
    Video {
        /// Filter used to select the Video stream depending on the width frame size.
        filter_frame_width: ComparatorFilter,
        /// Filter used to select the Video stream depending on the height frame size.
        filter_frame_height: ComparatorFilter,
        // TODO add aspect ratio as filter?
        /// Out image type used to convert the video frames.
        out_image_type: OutPixelFormat,
        out_image_size: OutImageSize,
        /// Buffer size is used to determinate the max buffer size.
        /// The actual size of the buffer depends on the stream type; if the specified size is not enough
        /// to store even a frame the StreamReader can't be created and so the MovieReader is not
        /// created too.
        buffer_size_mb: usize,
    },
    // Audio, TODO support?
    // Subtitle, TODO support ?
}

impl StreamInfo {
    /// Returns a `StreamInfo` that takes the best video stream and outputs
    /// a texture of type `U8U8U8`.
    /// The created buffer size is 500mb.
    pub fn best_video() -> StreamInfo {
        StreamInfo::Video {
            filter_frame_height: ComparatorFilter::Greater,
            filter_frame_width: ComparatorFilter::Greater,
            out_image_type: OutPixelFormat::Specific(video_reader::PixelFormat::U8U8U8),
            out_image_size: OutImageSize::Original,
            buffer_size_mb: 500,
        }
    }
}

/// This enum is used to specify the comparator method that you want to use for a specific filter.
#[derive(Copy, Clone, Debug)]
pub enum ComparatorFilter {
    /// Not checked
    Irrelevant,
    /// Take the one with the value closest to this one.
    Nearest(f32),
    /// Take the smallest
    Smallest,
    /// Take the greatest
    Greater,
}

impl ComparatorFilter {
    pub fn other_fits_better(self, current: f32, other: f32) -> bool {
        match self {
            ComparatorFilter::Irrelevant => false,
            ComparatorFilter::Nearest(target) => {
                let r1 = (current - target).abs();
                let r2 = (other - target).abs();
                r1 > r2
            }
            ComparatorFilter::Smallest => current > other,
            ComparatorFilter::Greater => current < other,
        }
    }
}

/// This trait must be implemented by any stream reader
pub trait StreamReader {
    /// Returns the stream id
    fn stream_id(&self) -> usize;

    /// Returns the stream base time
    fn stream_time_base(&self) -> f64;

    /// Returns the stream duration
    fn stream_duration(&self) -> f64;

    /// This function is supposed to read ad acquire information of a packet.
    ///
    /// When a buffer is full it have return `ReadingResult::BufferFull`, the daemon will
    /// call again this function after a while.
    /// It will retry until this function returns `ReadingResult::Done`.
    fn read_packet(&mut self, packet: &ffmpeg::AVPacket) -> Result<ReadingResult, String>;

    /// This function is called when the daemon is going to end its execution. This can happen anytime
    /// even if not the last frame is reached.
    ///
    /// Here you want to do the closing actions, like flushing the buffer.
    fn finalize(&mut self);

    /// Clear internal buffer
    fn clear_internal_buffer(&mut self);
}

/// The reading state
#[derive(Copy, Clone, Debug)]
pub enum ReadingResult {
    /// Returned when a specific reader is done with this packet
    Done,
    /// Returned when a specific reader can't process that packet right now but need to process it.
    BufferFull,
}

/// Voidable datas
pub trait Voidable {
    /// When true a data is void and have to be skipped.
    fn is_void(&self) -> bool;
}

/// The `StreamBuffer` is a lock free FIFO ring buffer, that pre allocates all the required memory.
/// This memory can be written and read safely at the same time by two different threads.
/// We have a single writer and a single reader where you can use the `StreamBufferProducer` to write,
/// and `StreamBufferConsumer` to read.
///
/// Usually the **producer** object is held by the `StreamReader` and the **consumer** is held by the client.
///
/// This is not a simple FIFO lock free ring buffer, it has some features to highlight:
/// - 1. Once created, it allocates the required memory and never frees it.
/// In this way, if we store a really big vector we can reuse this memory location.
///
/// - 2. Another feature of this buffer is to have a batch sorting mechanism;
/// before to make an element available to client, it is inserted inside a 
/// `sort_buffer`. When the function `flush` is called all the elements in the 
/// `sort_buffer` get sorted and then sent to the client.
#[derive(Debug)]
pub struct StreamBuffer {}

impl StreamBuffer {
    /// Creates a new `StreamBuffer`
    #[allow(clippy::new_ret_no_self)]
    pub fn new<T, F>(
        element_count: usize,
        f: F,
    ) -> (StreamBufferProducer<T>, StreamBufferConsumer<T>)
    where
        T: std::cmp::PartialOrd + Voidable,
        F: Fn() -> T,
    {
        let (buffer_producer, buffer_consumer) = RingBuffer::new(element_count).split();
        let (mut free_frames_producer, free_frames_consumer) =
            RingBuffer::new(element_count).split();

        free_frames_producer.push_each(|| -> Option<T> { Some(f()) });

        let producer = StreamBufferProducer {
            free_buffer_consumer: free_frames_consumer,
            sort_buffer: VecDeque::with_capacity(element_count),
            filling_in_process: None,
            buffer_producer,
        };

        let consumer = StreamBufferConsumer {
            free_buffer_producer: free_frames_producer,
            buffer_consumer,
            reading_in_process: None,
        };

        (producer, consumer)
    }
}

/// The producer of the `StreamBuffer` can be used to insert new data into the buffer.
/// The producer send only sorted data to the consumer.
/// This object is used by the `StreamReader`.
#[allow(missing_debug_implementations)]
pub struct StreamBufferProducer<T: std::cmp::PartialOrd + Voidable> {
    // This buffer is used to store the not yet used data.
    free_buffer_consumer: Consumer<T>,
    // The packets may arrive unordered, so when the push is finalized with `finalize_push`
    // it's inserted inside this buffer.
    // At some point, the `StreamReader` have to call `flush` that sorts what's inside the `sort_buffer`
    // and insert the data inside the buffer allowing the client to obtain the data.
    //
    // The size of this buffer depends a lot on the codification.
    sort_buffer: VecDeque<T>,
    // This buffer is the actual `StreamBuffer` that contains the data.
    buffer_producer: Producer<T>,
    filling_in_process: Option<T>,
}

impl<T: std::cmp::PartialOrd + Voidable> StreamBufferProducer<T> {
    /// Returns the capacity of the buffer
    pub fn capacity(&self) -> usize {
        self.buffer_producer.capacity()
    }

    /// Returns the actual data in the buffer
    pub fn len(&self) -> usize {
        self.buffer_producer.len() + self.sort_buffer.len()
    }

    /// Is the buffer empty?
    pub fn is_empty(&self) -> bool {
        self.free_buffer_consumer.is_full()
    }

    /// Is the buffer full?
    pub fn is_full(&self) -> bool {
        self.free_buffer_consumer.is_empty()
    }

    /// Returns the remaining free elements
    pub fn remaining(&self) -> usize {
        self.free_buffer_consumer.len()
    }

    /// Returns the number of elements are sent to the consumer
    pub fn sent_to_consumer(&self) -> usize {
        self.buffer_producer.len()
    }

    /// Returns the sort buffer
    pub fn sort_buffer_mut(&mut self) -> &mut VecDeque<T> {
        &mut self.sort_buffer
    }

    /// Returns a mutable reference of the inserted data.
    pub fn push(&mut self) -> Option<&mut T> {
        if self.filling_in_process.is_some() {
            panic!("Before to allocate a new element in the buffer you have to finalize the previous one");
        }
        self.filling_in_process = self.free_buffer_consumer.pop();
        self.filling_in_process.as_mut()
    }

    /// Finalize the push inserting it in the `sort_buffer`. The data is not yet
    /// available to the consumer, call `flush` to send it to the consumer.
    pub fn finalize_push(&mut self) {
        if let Some(data) = self.filling_in_process.take() {
            self.sort_buffer.push_back(data);
        }
    }

    /// Sort the data inside the `sort_buffer` and send it to the consumer
    ///
    /// This function is really useful to keep the buffer (for the consumer) sorted
    /// and ready to be use.
    pub fn flush(&mut self) {
        // Sort the entire buffer
        let (s1, s2) = self.sort_buffer.as_mut_slices();
        s1.sort_by(|v1, v2| -> std::cmp::Ordering { v1.partial_cmp(v2).unwrap() });

        // The slice two is always empty because this Ring buffer is used in a way
        // that never split the internal buffer
        assert!(s2.is_empty());

        loop {
            let d = self.sort_buffer.pop_front();
            if let Some(d) = d {
                if self.buffer_producer.push(d).is_err() {
                    unreachable!();
                }
            } else {
                break;
            }
        }

        // This reset the `head` and `tail` parameters to default.
        self.sort_buffer.clear();
    }
}

/// The consumer of the `StreamBuffer` is used by the client to get information from the buffer.
///
/// This object is never directly exposed to the user, but is wrapped by another class that give carry
/// some extra information.
/// For example the `VideoStreamReader`, returns a pointer to the `Video` object that contains the
/// buffer consumer, the frame size, the pixel format.
#[allow(missing_debug_implementations)]
pub struct StreamBufferConsumer<T: std::cmp::PartialOrd + Voidable> {
    free_buffer_producer: Producer<T>,
    buffer_consumer: Consumer<T>,
    reading_in_process: Option<T>,
}

impl<T: std::cmp::PartialOrd + Voidable> StreamBufferConsumer<T> {
    /// The capacity of the buffer
    pub fn capacity(&self) -> usize {
        self.buffer_consumer.capacity()
    }

    /// The number of elements in the buffer
    ///
    /// This value may change immediately.
    pub fn len(&self) -> usize {
        self.buffer_consumer.len()
    }

    /// Is the buffer empty?
    pub fn is_empty(&self) -> bool {
        self.buffer_consumer.is_empty()
    }

    /// The remaining space in the buffer.
    ///
    /// This value may change immediately.
    pub fn remaining(&self) -> usize {
        self.buffer_consumer.remaining()
    }

    /// Removes all the elements that this buffer currently have.
    ///
    /// Note: The memory is not deallocate, rather is returned to the `StreamBuffer` that can re-use it.
    pub fn clear(&mut self) {
        if self.reading_in_process.is_some() {
            panic!(
                "You can clear the buffer while you are reading a data. Call `finalize_pop` first"
            );
        }
        let fbo = &mut self.free_buffer_producer;
        self.buffer_consumer.pop_each(
            |d| -> bool {
                if fbo.push(d).is_err() {
                    unreachable!();
                }
                true
            },
            None,
        );
    }

    /// Returns Some with a valid reference to the buffered data
    ///
    /// Once done, is necessary to call `finalize_pop` to return the data into the buffer.
    pub fn pop(&mut self) -> Option<&T> {
        if self.reading_in_process.is_some() {
            panic!("Before pop another value you have to finalize the previous pop by calling `finalize_pop`");
        }

        let rip = &mut self.reading_in_process;
        let fbp = &mut self.free_buffer_producer;

        // Takes the first non void data and frees all the void data.
        self.buffer_consumer.pop_each(
            |v| -> bool {
                if v.is_void() {
                    if fbp.push(v).is_err() {
                        unreachable!();
                    }
                    true
                } else {
                    rip.replace(v);
                    false
                }
            },
            None,
        );

        rip.as_ref()
    }

    /// Mark the `pop` memory as free and returns it to the `StreamReader` that can re-use it.
    ///
    /// Must be **always** called after `pop` once done with the data.
    pub fn finalize_pop(&mut self) {
        if let Some(data) = self.reading_in_process.take() {
            if self.free_buffer_producer.push(data).is_err() {
                unreachable!();
            }
        }
    }
}

pub mod video_reader;

#[cfg(test)]
mod test {
    use crate::stream_reader::{video_reader::BufferedFrame, StreamBuffer};

    #[test]
    fn stream_buffer_memory() {
        let element_count = 10;
        let element_count_1_third = 3;

        let (mut buffer_producer, buffer_consumer) =
            StreamBuffer::new(element_count, || -> BufferedFrame {
                BufferedFrame::with_capacity(100)
            });

        assert_eq!(buffer_producer.capacity(), buffer_consumer.capacity());

        assert_eq!(buffer_producer.capacity(), element_count);
        assert_eq!(buffer_producer.len(), 0);
        assert_eq!(buffer_producer.sent_to_consumer(), 0);
        assert_eq!(buffer_producer.remaining(), element_count);
        assert_eq!(buffer_producer.is_full(), false);

        // Insert 1/3 elements to the sort buffer
        for x in 0..element_count_1_third {
            if let Some(ff) = buffer_producer.push() {
                ff.timestamp = x as f64;
            }
            buffer_producer.finalize_push()
        }

        // Check the producer
        assert_eq!(buffer_producer.capacity(), element_count);
        assert_eq!(buffer_producer.len(), element_count_1_third);
        assert_eq!(buffer_producer.sent_to_consumer(), 0);
        assert_eq!(
            buffer_producer.remaining(),
            element_count - element_count_1_third
        );
        assert_eq!(buffer_producer.is_full(), false);

        // Check the consumer
        assert_eq!(buffer_consumer.len(), buffer_producer.sent_to_consumer());
        assert_eq!(buffer_consumer.remaining(), element_count);

        // Send the data sorted to the client
        buffer_producer.flush();

        // Check the producer
        assert_eq!(buffer_producer.capacity(), element_count);
        assert_eq!(buffer_producer.len(), element_count_1_third);
        assert_eq!(buffer_producer.sent_to_consumer(), element_count_1_third);
        assert_eq!(
            buffer_producer.remaining(),
            element_count - element_count_1_third
        );
        assert_eq!(buffer_producer.is_full(), false);

        // Check the consumer
        assert_eq!(buffer_consumer.len(), buffer_producer.sent_to_consumer());
        assert_eq!(
            buffer_consumer.remaining(),
            element_count - element_count_1_third
        );
    }

    #[test]
    fn stream_buffer_clear() {
        let element_count = 10;
        let element_count_1_third = 3;

        let (mut buffer_producer, mut buffer_consumer) =
            StreamBuffer::new(element_count, || -> BufferedFrame {
                BufferedFrame::with_capacity(100)
            });

        // Insert 1/3 elements to the sort buffer
        for x in 0..element_count_1_third {
            if let Some(ff) = buffer_producer.push() {
                ff.timestamp = x as f64;
            }
            buffer_producer.finalize_push()
        }
        buffer_producer.flush();

        assert_eq!(buffer_producer.sent_to_consumer(), element_count_1_third);
        assert_eq!(buffer_consumer.len(), element_count_1_third);

        buffer_consumer.clear();

        // Check the producer
        assert_eq!(buffer_producer.len(), 0);
        assert_eq!(buffer_producer.sent_to_consumer(), 0);
        assert_eq!(buffer_producer.remaining(), element_count);

        // Check the consumer
        assert_eq!(buffer_consumer.len(), 0);
        assert_eq!(buffer_consumer.remaining(), element_count);
    }

    #[test]
    fn stream_buffer_pop_valid() {
        let element_count = 10;
        let element_count_1_third = 3;

        let (mut buffer_producer, mut buffer_consumer) =
            StreamBuffer::new(element_count, || -> BufferedFrame {
                BufferedFrame::with_capacity(100)
            });

        // Insert 1/3 elements with negative timestamp
        for x in 0..element_count {
            if let Some(ff) = buffer_producer.push() {
                ff.timestamp = x as f64 - element_count_1_third as f64;
            }
            buffer_producer.finalize_push()
        }
        buffer_producer.flush();

        // Make sure that all the elements got sent
        assert_eq!(buffer_producer.is_full(), true);
        assert_eq!(buffer_consumer.len(), element_count);

        // Pop the elements and assert that the timestamp is never negative.
        let mut count = 0;
        loop {
            let d = buffer_consumer.pop();
            if let Some(d) = d {
                assert!(d.timestamp >= 0.0);
                count += 1;
                buffer_consumer.finalize_pop();
            } else {
                break;
            }
        }

        // Assert that _void_ elements got skipped automatically.
        assert_eq!(count, element_count - element_count_1_third);

        assert_eq!(buffer_producer.is_full(), false);
        assert_eq!(buffer_consumer.len(), 0);
    }

    #[test]
    fn stream_buffer_order() {
        let element_count = 10;
        let element_count_1_third = 10;

        let (mut buffer_producer, mut buffer_consumer) =
            StreamBuffer::new(element_count, || -> BufferedFrame {
                BufferedFrame::with_capacity(100)
            });

        // Insert the elements with a decreasing timestamp
        for x in 0..element_count {
            if let Some(ff) = buffer_producer.push() {
                ff.timestamp = element_count as f64 - x as f64;
                ff.timestamp += 1.0; // Be sure that are always more than 0
                buffer_producer.finalize_push()
            }
        }
        buffer_producer.flush();

        // Pop the elements and assert that the timestamp is in ascending order
        let mut prev_timestamp = -1.0;
        loop {
            let d = buffer_consumer.pop();
            if let Some(d) = d {
                assert!(d.timestamp >= 0.0);
                assert!(d.timestamp >= prev_timestamp);
                prev_timestamp = d.timestamp;
                buffer_consumer.finalize_pop();
            } else {
                break;
            }
        }

        // The following code (which is similar to the above one) try to make the
        // internal ring buffer double ended (which is not something desirable
        // in this case).
        for _ in 0..5 {
            // Insert the elements with a decreasing timestamp
            for x in 0..element_count_1_third {
                if let Some(ff) = buffer_producer.push() {
                    ff.timestamp = element_count as f64 - x as f64;
                    ff.timestamp += 1.0; // Be sure that are always more than 0
                    buffer_producer.finalize_push();
                }
            }
            buffer_producer.flush();

            // Pop the elements and assert that the timestamp is in ascending order
            let mut prev_timestamp = -1.0;
            loop {
                let d = buffer_consumer.pop();
                if let Some(d) = d {
                    assert!(d.timestamp >= 0.0);
                    assert!(d.timestamp >= prev_timestamp);
                    prev_timestamp = d.timestamp;
                    buffer_consumer.finalize_pop();
                } else {
                    break;
                }
            }
        }
    }
}
