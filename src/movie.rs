use std::{ffi::CString, path::Path};

use ffmpeg_sys as ffmpeg;

/// Struct that safely wraps the opened FFmpeg input movie.
#[derive(Debug)]
pub struct Movie {
    context: *mut ffmpeg::AVFormatContext,
}

impl Movie {
    pub(crate) fn try_new(movie_path: &Path) -> Result<Self, String> {
        unsafe {
            // Open the video
            let mut video_input_context: *mut ffmpeg::AVFormatContext = std::ptr::null_mut();
            let cs_movie_path = CString::new(
                movie_path
                    .to_str()
                    .expect("Movie path specified is invalid"),
            )
            .unwrap();

            if ffmpeg::avformat_open_input(
                &mut video_input_context,
                cs_movie_path.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) != 0
            {
                return Err(format!(
                    "File opening failed: {}",
                    movie_path.to_str().unwrap()
                ));
            }

            let m = Movie {
                context: video_input_context,
            };

            // Read the stream information from the file.
            if ffmpeg::avformat_find_stream_info(m.context, std::ptr::null_mut()) < 0 {
                return Err("Can't read video stream information.".to_owned());
            }

            Ok(m)
        }
    }

    /// Returns the FFmpeg input context
    ///
    /// This function is unsafe because returns a raw pointer
    pub unsafe fn context_ptr(&mut self) -> *mut ffmpeg::AVFormatContext {
        self.context
    }
}

impl Drop for Movie {
    fn drop(&mut self) {
        unsafe {
            ffmpeg::avformat_close_input(&mut self.context);
        }
    }
}
