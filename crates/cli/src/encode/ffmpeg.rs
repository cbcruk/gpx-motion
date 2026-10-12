use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

/// Streams raw RGBA frames into ffmpeg over stdin and encodes H.264 MP4.
///
/// ffmpeg writes beside the output under a temporary name, which
/// [`FfmpegEncoder::finish`] renames into place. Dropped unfinished (an
/// error or panic mid-render), it stops ffmpeg and removes that file, so a
/// failed run leaves no partial video and keeps any earlier one.
pub struct FfmpegEncoder {
    child: Child,
    stdin: Option<ChildStdin>,
    partial: PathBuf,
    output: PathBuf,
    finished: bool,
}

impl FfmpegEncoder {
    pub fn start(output: &Path, width: u32, height: u32, fps: f64) -> io::Result<Self> {
        let partial = partial_path(output);
        let mut child = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y"])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{width}x{height}")])
            .args(["-r", &format!("{fps}")])
            .args(["-i", "pipe:0"])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18"])
            .args(["-movflags", "+faststart", "-f", "mp4"])
            .arg(&partial)
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => io::Error::new(
                    e.kind(),
                    "ffmpeg not found on PATH (it is needed to encode MP4)",
                ),
                _ => io::Error::new(e.kind(), format!("could not start ffmpeg: {e}")),
            })?;
        let stdin = child.stdin.take();
        let encoder = Self {
            child,
            stdin,
            partial,
            output: output.to_path_buf(),
            finished: false,
        };
        if encoder.stdin.is_none() {
            return Err(io::Error::other("ffmpeg stdin unavailable"));
        }
        Ok(encoder)
    }

    pub fn write_frame(&mut self, rgba: &[u8]) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("ffmpeg stdin closed"))?;
        stdin.write_all(rgba).map_err(|e| match e.kind() {
            io::ErrorKind::BrokenPipe => {
                io::Error::other("ffmpeg stopped early (see its error above)")
            }
            _ => e,
        })
    }

    /// Close the pipe, wait for ffmpeg to finish encoding, and move the video into place.
    pub fn finish(mut self) -> io::Result<()> {
        drop(self.stdin.take());
        let status = self.child.wait()?;
        if !status.success() {
            return Err(io::Error::other(format!("ffmpeg exited with {status}")));
        }
        fs::rename(&self.partial, &self.output).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "{}: could not move the video into place: {e}",
                    self.output.display()
                ),
            )
        })?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for FfmpegEncoder {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.partial);
    }
}

/// Where ffmpeg writes before the video is complete: a hidden file beside `output`.
fn partial_path(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .map_or_else(|| "output".into(), |n| n.to_string_lossy());
    output.with_file_name(format!(".{name}.partial"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_partial_file_sits_hidden_beside_the_output() {
        assert_eq!(
            partial_path(Path::new("out/ride.mp4")),
            Path::new("out/.ride.mp4.partial")
        );
        assert_eq!(
            partial_path(Path::new("ride.mp4")),
            Path::new(".ride.mp4.partial")
        );
    }
}
