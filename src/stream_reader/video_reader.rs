use ffmpeg_sys as ffmpeg;

use crate::{
    stream_reader::{
        ComparatorFilter, ReadingResult, StreamBuffer, StreamBufferConsumer, StreamBufferProducer,
        StreamReader, Voidable,
    },
    Movie,
};

/// The `VideoStreamReader` is a class used to decodify the video packets of a particular stream of
/// the `Movie` object.
///
/// Just after the decodification it is possible to convert the frame data, that is formatted following
/// the codec rules, to a format more suitable to our needs; and nonetheless, it's possible to also
/// scale the frames size.
///
/// # Undefined behavior
/// The object of this class is automatically created by the `Movie`, and this `VideoStreamReader`
/// can't be used to read the stream of another `Movie`.
pub(crate) struct VideoStreamReader {
    stream_id: usize,
    stream_time_base: f64,
    stream_duration: f64,
    biggest_time_stamp: f64,
    decoder_codec_ctx: *mut ffmpeg::AVCodecContext,

    // The `decoded_frame` is used as placeholder for the decoded frame. Each `AVFrame` needs a buffer,
    // but we don't need to assign one to this one because the decoder will provide one.
    //
    // The `Option` is used to implement the safety mechanism on the `VideoStreamReader` constructor,
    // so it's safe to assume that it's always `Some`.
    decoded_frame: Option<*mut ffmpeg::AVFrame>,

    // The `out_frame` is used to write the data in the main buffer. Indeed, the conversion_program
    // will write on this frame, that in turn will have a different buffer.
    //
    // For each frame conversion we assign directly the destination buffer, so we don't need to duplicate
    // the data after the conversion, rather the conversion program will write on the output buffer.
    //
    // The `Option` is used to implement the safety mechanism on the `VideoStreamReader` constructor,
    // so it's safe to assume that it's always `Some`.
    out_frame: Option<*mut ffmpeg::AVFrame>,
    out_pixel_format: PixelFormat,
    out_frame_size: [u32; 2],

    // The conversion program is used to mutate the decoded frame, to a format that fits better our
    // needs.
    //
    // The `Option` is used to implement the safety mechanism on the `VideoStreamReader` constructor,
    // so it's safe to assume that it's always `Some`.
    sws_conversion_program: Option<*mut ffmpeg::SwsContext>,

    // The buffer where to store the frame data
    //
    // The `Option` is used to implement the safety mechanism on the `VideoStreamReader` constructor,
    // so it's safe to assume that it's always `Some`.
    buffer: Option<StreamBufferProducer<BufferedFrame>>,
}

impl VideoStreamReader {
    pub(crate) fn try_new(
        movie: &mut Movie,
        filter_frame_width: ComparatorFilter,
        filter_frame_height: ComparatorFilter,
        out_image_type: OutPixelFormat,
        out_image_size: OutImageSize,
        buffer_size_mb: usize,
    ) -> Result<(Self, Video), String> {
        // Find the video stream depending on the submitted filters
        let stream_id =
            VideoStreamReader::chose_stream(movie, filter_frame_width, filter_frame_height)?;

        let (stream_time_base, stream_duration) =
            VideoStreamReader::gather_stream_info(movie, stream_id)?;

        // Creates a separate codec context, that is possible to use independently from the movie
        // input context.
        let decoder_codec_ctx = VideoStreamReader::copy_stream_codec_context(movie, stream_id)?;

        // Note: I'm composing this here, so if something go wrong from this moment on the destructor
        // of the `VideoStreamReader` will take care to clean out the memory.
        let mut vsr = VideoStreamReader {
            stream_id,
            stream_time_base,
            stream_duration,
            biggest_time_stamp: -1.0,
            decoder_codec_ctx,
            decoded_frame: None,
            out_frame: None,
            out_frame_size: [0, 0],
            out_pixel_format: PixelFormat::U8U8U8,
            sws_conversion_program: None,
            buffer: None,
        };

        unsafe {
            vsr.decoded_frame = Some(ffmpeg::av_frame_alloc());
            vsr.out_frame = Some(ffmpeg::av_frame_alloc());
        }

        let (p, frame_size, pixel_format) = VideoStreamReader::init_conversion_program(
            vsr.decoder_codec_ctx,
            out_image_type,
            out_image_size,
        )?;
        vsr.sws_conversion_program = Some(p);

        let (buffer_producer, buffer_consumer) =
            VideoStreamReader::create_buffer(frame_size, pixel_format, buffer_size_mb)?;

        vsr.buffer = Some(buffer_producer);
        vsr.out_frame_size = frame_size;
        vsr.out_pixel_format = pixel_format;

        Ok((
            vsr,
            Video {
                frame_size,
                pixel_format,
                duration: stream_duration,
                buffer: buffer_consumer,
            },
        ))
    }

    fn chose_stream(
        movie: &mut Movie,
        filter_frame_width: ComparatorFilter,
        filter_frame_height: ComparatorFilter,
    ) -> Result<usize, String> {
        unsafe {
            let stream_id = {
                let mut video_stream_id = -1isize; // Old style coding, Let's rock!
                let mut video_stream_width = 0i32;
                let mut video_stream_height = 0i32;
                let mut video_stream_filter_score = 0u32;

                for i in 0isize..(*movie.context_ptr()).nb_streams as isize {
                    let stream_codec = *(*(*(*movie.context_ptr()).streams.offset(i))).codec;

                    if stream_codec.codec_type != ffmpeg::AVMediaType::AVMEDIA_TYPE_VIDEO {
                        continue;
                    }

                    let mut score = 1u32;
                    if filter_frame_width
                        .other_fits_better(video_stream_width as f32, stream_codec.width as f32)
                    {
                        score += 1;
                    }

                    if filter_frame_height
                        .other_fits_better(video_stream_height as f32, stream_codec.height as f32)
                    {
                        score += 1;
                    }

                    if score > video_stream_filter_score {
                        video_stream_id = i;
                        video_stream_width = stream_codec.width;
                        video_stream_height = stream_codec.height;
                        video_stream_filter_score = score;
                    }
                }
                video_stream_id
            };

            if stream_id < 0 {
                Err("Was not possible to find any suitable video stream for this movie.".to_owned())
            } else {
                Ok(stream_id as usize)
            }
        }
    }

    // Returns a tuple that contains:
    // 0: stream_time_base
    // 1: stream_duration
    fn gather_stream_info(movie: &mut Movie, stream_id: usize) -> Result<(f64, f64), String> {
        unsafe {
            let stream = *(*(*movie.context_ptr()).streams.add(stream_id));
            let stream_time_base = ffmpeg::av_q2d(stream.time_base);
            let stream_duration = stream.duration as f64 * stream_time_base;
            let _stream_start_time = stream.start_time as f64 * stream_time_base; // We are not using this because of: https://www.ffmpeg.org/doxygen/2.0/structAVStream.html#a7c67ae70632c91df8b0f721658ec5377
            Ok((stream_time_base, stream_duration))
        }
    }

    fn copy_stream_codec_context(
        movie: &mut Movie,
        stream_id: usize,
    ) -> Result<*mut ffmpeg::AVCodecContext, String> {
        unsafe {
            let video_stream_codec_ctx = (*(*(*movie.context_ptr()).streams.add(stream_id))).codec;

            // Take the decoder from the ID.
            let video_codec = ffmpeg::avcodec_find_decoder((*video_stream_codec_ctx).codec_id);
            if video_codec.is_null() {
                return Err(format!(
                    "The decoder of this codec `{:?}` was not found.",
                    (*video_stream_codec_ctx).codec_id
                ));
            }

            // Now, duplicate the codec context
            let mut decoder_codec_ctx = {
                let video_codec_ctx = ffmpeg::avcodec_alloc_context3(video_codec);
                let mut params = ffmpeg::avcodec_parameters_alloc();
                ffmpeg::avcodec_parameters_from_context(params, video_stream_codec_ctx);
                ffmpeg::avcodec_parameters_to_context(video_codec_ctx, params);
                ffmpeg::avcodec_parameters_free(&mut params);
                video_codec_ctx
            };

            // Open the new decoder codec context
            if ffmpeg::avcodec_open2(decoder_codec_ctx, video_codec, std::ptr::null_mut()) < 0 {
                ffmpeg::avcodec_free_context(&mut decoder_codec_ctx);
                return Err("The newly created codex context can't be opened.".to_owned());
            }

            Ok(decoder_codec_ctx)
        }
    }

    #[allow(clippy::infallible_destructuring_match)]
    fn init_conversion_program(
        codec_ctx: *mut ffmpeg::AVCodecContext,
        out_image_type: OutPixelFormat,
        out_image_size: OutImageSize,
    ) -> Result<(*mut ffmpeg::SwsContext, [u32; 2], PixelFormat), String> {
        unsafe {
            let format = match out_image_type {
                // OutImageType::Original => (*codec_ctx).pix_fmt,
                OutPixelFormat::Specific(pixel_format) => pixel_format,
            };
            let (w, h, algorithm) = match out_image_size {
                OutImageSize::Original => (
                    (*codec_ctx).width,
                    (*codec_ctx).height,
                    ffmpeg::SWS_FAST_BILINEAR,
                ),
                OutImageSize::Scale(s, algorithm) => {
                    let algorithm = match algorithm {
                        ImageOptimizationAlgorithm::FastBilinear => ffmpeg::SWS_FAST_BILINEAR,
                        ImageOptimizationAlgorithm::Bilinear => ffmpeg::SWS_BILINEAR,
                        ImageOptimizationAlgorithm::Bicubic => ffmpeg::SWS_BICUBIC,
                        ImageOptimizationAlgorithm::Gauss => ffmpeg::SWS_GAUSS,
                    };
                    (s[0] as i32, s[1] as i32, algorithm)
                }
            };

            let sws_context = ffmpeg::sws_getContext(
                (*codec_ctx).width,     // Source
                (*codec_ctx).height,    // Source
                (*codec_ctx).pix_fmt,   // Source
                w,                      // Target
                h,                      // Target
                format.ffmpeg_format(), // Target
                algorithm,              // Target
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );

            Ok((sws_context, [w as u32, h as u32], format))
        }
    }

    fn create_buffer(
        frame_size: [u32; 2],
        pixel_format: PixelFormat,
        max_size_mb: usize,
    ) -> Result<
        (
            StreamBufferProducer<BufferedFrame>,
            StreamBufferConsumer<BufferedFrame>,
        ),
        String,
    > {
        let frame_buffer_bytes = unsafe {
            ffmpeg::av_image_get_buffer_size(
                pixel_format.ffmpeg_format(),
                frame_size[0] as i32,
                frame_size[1] as i32,
                1, // See this: https://stackoverflow.com/a/35837289/2653173
            ) as usize
        };

        let frame_buffer_mb = frame_buffer_bytes as f32 / (1024f32 * 1024f32);
        if frame_buffer_mb <= 0.0 {
            return Err("Each frame takes 0 space, are you sure that the specified frame sizes are correct?".to_owned());
        }

        let frames_count = (max_size_mb as f32 / frame_buffer_mb).floor() as usize;

        if frames_count == 0 {
            return Err(format!("The specified buffer max size is not enough to even store a frame; a single frame size is {}mb", frame_buffer_mb));
        }

        Ok(StreamBuffer::new(frames_count, || -> BufferedFrame {
            BufferedFrame::with_capacity(frame_buffer_bytes)
        }))
    }
}

impl Drop for VideoStreamReader {
    fn drop(&mut self) {
        unsafe {
            // Check if `Some` because the initialization may fail when not all of them are assigned
            if let Some(f) = self.decoded_frame {
                ffmpeg::av_free(f as *mut std::ffi::c_void);
            }

            if let Some(f) = self.out_frame {
                ffmpeg::av_free(f as *mut std::ffi::c_void);
            }

            if let Some(cp) = self.sws_conversion_program {
                ffmpeg::sws_freeContext(cp);
            }

            ffmpeg::avcodec_close(self.decoder_codec_ctx);
            ffmpeg::avcodec_free_context(&mut self.decoder_codec_ctx);

            self.decoded_frame = None;
            self.out_frame = None;
            self.sws_conversion_program = None;
            self.buffer = None;
        }
    }
}

impl StreamReader for VideoStreamReader {
    fn stream_id(&self) -> usize {
        self.stream_id
    }

    fn stream_time_base(&self) -> f64 {
        self.stream_time_base
    }

    fn stream_duration(&self) -> f64 {
        self.stream_duration
    }

    fn read_packet(&mut self, packet: &ffmpeg::AVPacket) -> Result<ReadingResult, String> {
        if packet.stream_index == self.stream_id as i32 {
            unsafe {
                // Request a buffer memory location
                //if let Some(buffered_frame) = self.buffer.as_mut().unwrap().push() {
                if !self.buffer.as_ref().unwrap().is_full() {
                    // Set the packet to decode
                    if ffmpeg::avcodec_send_packet(self.decoder_codec_ctx, packet) < 0 {
                        return Err("Can't decode this packet.".to_owned());
                    }

                    // We could receive 0 or N frames, however we are just taking
                    // the first one or nothing.
                    if ffmpeg::avcodec_receive_frame(
                        self.decoder_codec_ctx,
                        self.decoded_frame.unwrap(),
                    ) >= 0
                    {
                        // Packet received with data, so push it into the buffer.
                        let buffered_frame = self
                            .buffer
                            .as_mut()
                            .unwrap()
                            .push()
                            .expect("Checked before, so unreachable");

                        // Sets the `buffered_frame.data` as buffer of the
                        // `self.out_frame` so the `sws_scale` program can directly
                        // write on it.
                        if ffmpeg::avpicture_fill(
                            self.out_frame.unwrap() as *mut ffmpeg::AVPicture, // Safe cast (Check FFmpeg docs).
                            buffered_frame.data.as_mut_ptr(), // This vector is already allocated during the buffer initialization.
                            self.out_pixel_format.ffmpeg_format(),
                            self.out_frame_size[0] as i32,
                            self.out_frame_size[1] as i32,
                        ) <= 0
                        {
                            buffered_frame.timestamp = -1.0; // Invalidate the frame
                            self.buffer.as_mut().unwrap().finalize_push();
                            return Err(
                                "Was not possible to assign the buffer to the `out_buffer`."
                                    .to_owned(),
                            );
                        }

                        // Convert the frame
                        ffmpeg::sws_scale(
                            self.sws_conversion_program.unwrap(),
                            (*self.decoded_frame.unwrap()).data.as_ptr() as *const *const _,
                            (*self.decoded_frame.unwrap()).linesize.as_ptr() as *mut _,
                            0,
                            (*self.decoded_frame.unwrap()).height,
                            (*self.out_frame.unwrap()).data.as_ptr() as *const *const _,
                            (*self.out_frame.unwrap()).linesize.as_ptr() as *mut _,
                        );

                        // Remove the buffer from the frame, (It's not really necessary but it's better to do).
                        if ffmpeg::avpicture_fill(
                            self.out_frame.unwrap() as *mut ffmpeg::AVPicture, // Safe cast (Check FFmpeg docs).
                            std::ptr::null_mut(),
                            self.out_pixel_format.ffmpeg_format(),
                            self.out_frame_size[0] as i32,
                            self.out_frame_size[1] as i32,
                        ) <= 0
                        {
                            buffered_frame.timestamp = -1.0; // Invalidate the frame
                            self.buffer.as_mut().unwrap().finalize_push();
                            return Err(
                                "Was not possible to de-assign buffer from the `out_frame`."
                                    .to_owned(),
                            );
                        }

                        // Store the converted frame presentation timestamp; I'm using the function
                        // `av_frame_get_best_effort_timestamp` because not all the decoder sets a PTS,
                        // so this function take care to return a consistent timestamp.
                        let timestamp =
                            ffmpeg::av_frame_get_best_effort_timestamp(self.decoded_frame.unwrap())
                                as f64
                                * self.stream_time_base;

                        buffered_frame.timestamp = timestamp;

                        // If the timestamp of the new packet is more than the biggest timestamp read until now
                        // flush the buffer.
                        // Flush the buffer mean, sort the collected frames and make available to the client.
                        if self.biggest_time_stamp < timestamp {
                            self.buffer.as_mut().unwrap().flush();
                            self.biggest_time_stamp = timestamp;
                        }

                        // Finally push the data into the `sort_buffer`
                        self.buffer.as_mut().unwrap().finalize_push();
                    } else {
                        println!("Received 0 packets! We would be really glad if you could open an issue.")
                    }
                } else {
                    // The buffer is completely full
                    return Ok(ReadingResult::BufferFull);
                }
            }
        }

        Ok(ReadingResult::Done)
    }

    fn finalize(&mut self) {
        self.buffer.as_mut().unwrap().flush();
    }

    fn clear_internal_buffer(&mut self) {
        // The only way to put this frames back into the free buffer for the `StreamBufferProducer` is
        // to flush them.
        // Before doing it, we mark these frames as invalid by setting the timestamp to -1
        for f in self.buffer.as_mut().unwrap().sort_buffer_mut() {
            f.timestamp = -1.0;
        }
        self.buffer.as_mut().unwrap().flush();
    }
}

/// Output image type
#[derive(Copy, Clone, Debug)]
pub enum OutPixelFormat {
    // Removed because if I do so I have to make sure that the buffer is able to allocate the correct
    // space for any ffmpeg supported types (that are a lot and useless for my currently needs).
    // /// Leaves the data untouched
    // Original,
    /// Specific pixel format
    Specific(PixelFormat),
}

/// Image type
#[derive(Copy, Clone, Debug)]
pub enum PixelFormat {
    /// RGB24
    U8U8U8,
}

impl PixelFormat {
    pub fn ffmpeg_format(self) -> ffmpeg::AVPixelFormat {
        match self {
            PixelFormat::U8U8U8 => ffmpeg::AVPixelFormat::AV_PIX_FMT_RGB24,
        }
    }
}

/// Output image size
#[derive(Copy, Clone, Debug)]
pub enum OutImageSize {
    /// Leaves the frame size untouched
    Original,
    /// Scale the frame size to this exact dimensions.
    /// The second parameter is used to specify the image optimization algorithm to use.
    Scale([u32; 2], ImageOptimizationAlgorithm),
}

/// Available image optimization algorithms
#[derive(Copy, Clone, Debug)]
pub enum ImageOptimizationAlgorithm {
    FastBilinear,
    Bilinear,
    Bicubic,
    Gauss,
}

pub struct Video {
    frame_size: [u32; 2],
    pixel_format: PixelFormat,
    duration: f64,
    buffer: StreamBufferConsumer<BufferedFrame>,
}

impl Video {
    pub fn frame_size(&self) -> [u32; 2] {
        self.frame_size
    }

    pub fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn buffer(&mut self) -> &mut StreamBufferConsumer<BufferedFrame> {
        &mut self.buffer
    }
}

impl std::fmt::Debug for Video {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f,
            "Video {{\n    Frame width: {} height: {}\n    Pixel format: {:?}\n    Duration: {}\n}}",
            self.frame_size[0],
            self.frame_size[1],
            self.pixel_format,
            self.duration,
        )
    }
}

/// The `BufferedFrame` stores useful information about the image to show on screen.
#[derive(Clone, Debug)]
pub struct BufferedFrame {
    /// This timestamp in second, tells when this frame must be show in video.
    pub timestamp: f64,
    /// The image data
    pub data: Vec<u8>,
}

impl BufferedFrame {
    /// Creates a `BufferedFrame` with specific capacity
    ///
    /// ```rust
    /// use video_ludo::stream_reader::video_reader::BufferedFrame;
    ///
    /// let f = BufferedFrame::with_capacity(10);
    /// assert_eq!(10, f.data().len());
    /// ```
    pub fn with_capacity(data_size: usize) -> Self {
        let mut data = vec![0; data_size];
        data.shrink_to_fit();
        BufferedFrame {
            timestamp: -1.0,
            data,
        }
    }

    /// Clone the data from another `BufferedFrame` into this pre allocated one.
    pub fn clone_from(&mut self, other: &Self) {
        self.timestamp = other.timestamp;
        self.data.clone_from(&other.data);
    }

    pub fn timestamp(&self) -> f64 {
        self.timestamp
    }

    pub fn data(&self) -> &Vec<u8> {
        &self.data
    }
}

impl Default for BufferedFrame {
    fn default() -> Self {
        BufferedFrame::with_capacity(0)
    }
}

impl Voidable for BufferedFrame {
    fn is_void(&self) -> bool {
        self.timestamp < 0.0
    }
}

impl std::cmp::PartialOrd for BufferedFrame {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.timestamp.partial_cmp(&other.timestamp)
    }
}

impl std::cmp::PartialEq for BufferedFrame {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp.eq(&other.timestamp)
    }
}

#[cfg(test)]
mod test {

    use crate::stream_reader::{video_reader::BufferedFrame, Voidable};

    #[test]
    fn buffered_frame_allocation() {
        let bf = BufferedFrame::with_capacity(1000);
        assert_eq!(bf.data.len(), 1000);
    }

    #[test]
    fn buffered_frame_voidable() {
        let mut bf = BufferedFrame::default();
        assert_eq!(bf.is_void(), true);

        bf.timestamp = 0.001;
        assert_eq!(bf.is_void(), false);

        bf.timestamp = -0.001;
        assert_eq!(bf.is_void(), true);
    }
}
