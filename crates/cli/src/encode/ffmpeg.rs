use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

/// Streams raw RGBA frames into ffmpeg over stdin and encodes H.264 MP4.
pub struct FfmpegEncoder {
    child: Child,
    stdin: ChildStdin,
}

impl FfmpegEncoder {
    pub fn start(output: &Path, width: u32, height: u32, fps: f64) -> io::Result<Self> {
        let mut child = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y"])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{width}x{height}")])
            .args(["-r", &format!("{fps}")])
            .args(["-i", "pipe:0"])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18"])
            .arg("-movflags")
            .arg("+faststart")
            .arg(output)
            .stdin(Stdio::piped())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("ffmpeg stdin unavailable"))?;
        Ok(Self { child, stdin })
    }

    pub fn write_frame(&mut self, rgba: &[u8]) -> io::Result<()> {
        self.stdin.write_all(rgba)
    }

    /// Close the pipe and wait for ffmpeg to finish encoding.
    pub fn finish(mut self) -> io::Result<()> {
        drop(self.stdin);
        let status = self.child.wait()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("ffmpeg exited with {status}")))
        }
    }
}
