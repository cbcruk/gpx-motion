//! Video encoders the rendered frames are streamed into.

pub mod ffmpeg;

pub use ffmpeg::FfmpegEncoder;
