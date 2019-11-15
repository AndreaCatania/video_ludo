[![Video Ludo logo](/logo.png)](https://github.com/AndreaCatania/video_ludo)

# Video Ludo

[![Build Status]](https://travis-ci.org/AndreaCatania/video_ludo) [![License]](https://github.com/AndreaCatania/video_ludo/blob/master/LICENSE) [![Line of code]](https://github.com/AndreaCatania/video_ludo/pulse)

[Build Status]: https://travis-ci.org/AndreaCatania/video_ludo.svg?branch=master
[License]: https://img.shields.io/badge/License-MIT-green.svg
[Line of code]: https://tokei.rs/b1/github/andreacatania/video_ludo?category=code

The Video Ludo crate is a movie reader, written in rust Lang, which allows extract
information from the various streams that compose a movie file.

# Movie file
A _.mp4_ or _.ogv_ (etc..) are containers of video, audio, subtitles;
each of these is a separate stream, and these all together compose a reproducible movie file.

Is common to have a movie file with many languages subtitles, many languages audio,
and why not? some video resolutions.

Each of these are a separate streams, this mean that a file can have many streams
of the same type.

# Use Video Ludo
Video Ludo reads as many streams as you desire and converts the codified stream data into something
directly usable by the computer; this task is done in a separate thread, and you
will be able to retrieve these information using a `StreamReaderEntry`.

Even if it's simple to use let's go in order.

#### Step 1
The first thing to do is to construct
a list of `StreamInfo`, that you want to read.

```rust
let mut stream_info = Vec::new();
// Take the best resolution video stream and output the frame in RGB format (u8u8u8).
stream_info.push(StreamInfo::best_video())
```

#### Step 2
Now let's create the `MovieReader`.

```rust
let (mut movie_reader, mut stream_entries) = MovieReader::try_new(Path::new("resources/Ettore.ogv"), stream_info)
        .expect("Movie Reader creation fail!");
```

In this line I'm passing the path to the movie file and the previously constructed
`StreamInfo` list. As return I get an object of the `MovieReader` and a list of
`StreamReaderEntry`.

- `MovieReader`: Allow to control the reading process (play, stop, seek).
- `StreamReaderEntry` Allow to obtain the information that the `StreamReader` take from the specified stream.

Internally the `MovieReader` creates a `StreamReader` per each passed `StreamInfo`.
A `StreamReader` read the stream information, decodify it, and convert the data
to the needed raw format, and store it in a buffer.

In our case we are reading the best available video stream and to retrieve the
stored frames we can use the returned `StreamReaderEntry` object to access the buffer.

#### Step 3
The returned entry object must be casted from `Any` to `Video`:
```rust
let mut video = stream_entries.pop().unwrap().downcast::<Video>().unwrap();
```
It gives the possibility to know the stream infromation and retrieve the frames.

#### Step 4
We just need to start the file reading.
```rust
movie_reader.start_read();
```

**That's it!** you started the reading in a separate thread.

#### Get data
To retrieve the data from the `video` entry, we can use the `pop` function:
```rust
video.buffer_mut().pop();
```

This function, returns `Some(&BufferedFrame)` only when there is something, you can
read the data from this reference, this data is available untill the function
`finalize_pop` is called.

Call `finalize_pop` is necessary, and what it simply does is to return back the data
piece of memory.

# Stream Buffer
The buffer size of each `StreamReader` is fixed; when the buffer is full it doesn't
accept any new data untill you call `finalize_pop`.
In addition, each data (like the `BufferedFrame`) has a timestamp in seconds
(relative to the video) that can be used to correctly play the movie.
_The received data are sorted by timestamp._

# Example
You can find two examples, that demonstrates what I've just described.
You can notice that the video play by the examples is really fast.
This happens, because the frames are not sync to a timer; indeed, is not intention of
this crate implement a Movie **Player**, rather leaves to you the freedom to do it
(or not).

Example 1:
```
cargo run --example video_player
```

Example 2:
```
cargo run --example video_player_raw
```

# Contrubution
Any contribution is open; currently is missing the Audio and Subtitle `StreaReader`s
implementation, so any PR is welcome.