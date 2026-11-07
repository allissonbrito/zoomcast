//! Exporta MP4 aplicando `zoomcast_core::camera::camera_at_f` a cada frame.
//!
//! Pipeline: `ffmpeg` (decodifica, trim, `fps`, rgb24) -> Rust (recorte + bilinear) -> `ffmpeg`
//! (libx264 yuv420p, áudio aac recortado do original). O enquadramento do frame `k` usa
//! `t_ms = trim_start + k*1000/fps` em ponto flutuante.
mod frame;
mod probe;

pub use frame::{Rect, crop_rect, render_frame};
pub use probe::{ProbeInfo, probe};

use std::{
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread::JoinHandle,
};
use thiserror::Error;
use zoomcast_core::{camera::camera_at_f, project::Project, timeline::Timeline};

const STDERR_TAIL: usize = 2000;
const MAX_FPS: u32 = 240;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("opção inválida: {0}")]
    InvalidOptions(String),
    #[error("ffmpeg não encontrado em {0:?} (instale ou defina ZOOMCAST_FFMPEG)")]
    FfmpegNotFound(PathBuf),
    #[error("saída e entrada são o mesmo arquivo: {0}")]
    SameInputOutput(PathBuf),
    #[error("ffprobe: {0}")]
    Probe(String),
    #[error("ffmpeg ({stage}) falhou ({status}): {stderr}")]
    Ffmpeg {
        stage: &'static str,
        status: String,
        stderr: String,
    },
    #[error("projeto: {0}")]
    Core(#[from] zoomcast_core::Error),
    #[error("e/s: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    pub fps: u32,
    pub crf: u8,
    pub ffmpeg_bin: PathBuf,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            fps: 30,
            crf: 20,
            ffmpeg_bin: std::env::var_os("ZOOMCAST_FFMPEG")
                .filter(|v| !v.is_empty())
                .map_or_else(|| PathBuf::from("ffmpeg"), PathBuf::from),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    pub frames: u64,
    pub out_duration_ms: u64,
    pub out_width: u32,
    pub out_height: u32,
}

fn command(bin: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(bin);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

fn spawn_err(bin: &Path, e: io::Error) -> ExportError {
    if e.kind() == io::ErrorKind::NotFound {
        ExportError::FfmpegNotFound(bin.to_owned())
    } else {
        ExportError::Io(e)
    }
}

fn status_str(s: ExitStatus) -> String {
    s.code()
        .map_or_else(|| s.to_string(), |c| format!("código {c}"))
}

/// Executa `cmd` (stdout capturado, stderr limitado nos erros). Usado pelo ffprobe.
pub(crate) fn run_capture(bin: &Path, cmd: &mut Command) -> Result<Vec<u8>, ExportError> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| spawn_err(bin, e))?;
    if !out.status.success() {
        return Err(ExportError::Ffmpeg {
            stage: "probe",
            status: status_str(out.status),
            stderr: tail(&out.stderr),
        });
    }
    Ok(out.stdout)
}

fn tail(b: &[u8]) -> String {
    let b = &b[b.len().saturating_sub(STDERR_TAIL)..];
    String::from_utf8_lossy(b).trim().to_owned()
}

/// Drena stderr numa thread (evita deadlock de pipe cheio) guardando só o final.
fn drain(mut r: impl Read + Send + 'static) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let (mut buf, mut chunk) = (Vec::new(), [0u8; 4096]);
        while let Ok(n @ 1..) = r.read(&mut chunk) {
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > 2 * STDERR_TAIL {
                buf.drain(..buf.len() - STDERR_TAIL);
            }
        }
        tail(&buf)
    })
}

struct Proc {
    child: Child,
    stderr: Option<JoinHandle<String>>,
}

impl Proc {
    fn spawn(
        bin: &Path,
        mut cmd: Command,
        stdin: Stdio,
        stdout: Stdio,
    ) -> Result<Self, ExportError> {
        let mut child = cmd
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| spawn_err(bin, e))?;
        let stderr = child.stderr.take().map(drain);
        Ok(Self { child, stderr })
    }

    fn stderr(&mut self) -> String {
        self.stderr
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default()
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn finish(&mut self, stage: &'static str) -> Result<(), ExportError> {
        let status = self.child.wait()?;
        let stderr = self.stderr();
        if status.success() {
            Ok(())
        } else {
            Err(ExportError::Ffmpeg {
                stage,
                status: status_str(status),
                stderr,
            })
        }
    }
}

/// Retângulo de recorte do frame `k` da saída: câmera avaliada em `trim_start + k*1000/fps`.
#[must_use]
pub fn frame_rect(tl: &Timeline, trim_start_ms: u64, k: u64, fps: u32, w: u32, h: u32) -> Rect {
    let t_ms = trim_start_ms as f64 + k as f64 * 1000.0 / f64::from(fps);
    crop_rect(&camera_at_f(tl, t_ms), w, h)
}

fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| {
        p.canonicalize().or_else(|_| {
            let name = p.file_name().ok_or(io::ErrorKind::InvalidInput)?;
            let parent = p.parent().filter(|d| !d.as_os_str().is_empty());
            Ok::<_, io::Error>(parent.unwrap_or(Path::new(".")).canonicalize()?.join(name))
        })
    };
    match (canon(a), canon(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn os(s: impl Into<OsString>) -> OsString {
    s.into()
}

fn build_commands(
    opts: &ExportOptions,
    video: &Path,
    out: &Path,
    info: &ProbeInfo,
    (start, end): (u64, u64),
    total: u64,
) -> (Command, Command) {
    let (w, h) = (info.width, info.height);
    let (out_w, out_h) = (w & !1, h & !1);
    let mut dec_cmd = command(&opts.ffmpeg_bin);
    dec_cmd
        .args(["-hide_banner", "-loglevel", "error", "-ss"])
        .arg(format!("{start}ms"))
        .arg("-i")
        .arg(video)
        .args(["-an", "-vf"])
        .arg(format!("fps={}", opts.fps))
        .arg("-frames:v")
        .arg(total.to_string())
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"]);

    let mut enc_cmd = command(&opts.ffmpeg_bin);
    enc_cmd
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-s"])
        .arg(format!("{w}x{h}"))
        .arg("-framerate")
        .arg(opts.fps.to_string())
        .args(["-i", "pipe:0"]);
    if info.has_audio {
        enc_cmd
            .arg("-ss")
            .arg(format!("{start}ms"))
            .arg("-t")
            .arg(format!("{}ms", end - start))
            .arg("-i")
            .arg(video);
    }
    enc_cmd.args(["-map", "0:v:0"]);
    if info.has_audio {
        // Sem -shortest: áudio curto não pode truncar o vídeo;
        // o codificador para naturalmente quando o stream de frames (pipe:0) fecha.
        enc_cmd.args(["-map", "1:a:0", "-c:a", "aac", "-b:a", "192k"]);
    }
    enc_cmd
        .arg("-vf")
        .arg(format!("crop={out_w}:{out_h}:0:0"))
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf"])
        .arg(opts.crf.to_string())
        .args([
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
        .arg(os(out));
    (dec_cmd, enc_cmd)
}

/// Exporta `project` para `out` (MP4). `progress` recebe 0..=1.
///
/// # Errors
/// Opções inválidas, saída == entrada, ffmpeg/ffprobe ausente ou falho, trim vazio, e/s.
pub fn export_project(
    project: &Project,
    out: &Path,
    opts: &ExportOptions,
    progress: &mut dyn FnMut(f32),
) -> Result<ExportReport, ExportError> {
    if opts.fps == 0 || opts.fps > MAX_FPS {
        return Err(ExportError::InvalidOptions(format!(
            "fps deve estar em 1..={MAX_FPS}"
        )));
    }
    if opts.crf > 51 {
        return Err(ExportError::InvalidOptions(
            "crf deve estar em 0..=51".into(),
        ));
    }
    let video = project.video_file();
    if same_file(&video, out) {
        return Err(ExportError::SameInputOutput(video));
    }
    let info = probe(&opts.ffmpeg_bin, &video)?;
    let tl = &project.manifest.timeline;
    let (start, end) = tl.effective_trim(info.duration_ms)?;
    let (w, h) = (info.width, info.height);
    let (out_w, out_h) = (w & !1, h & !1);
    if out_w == 0 || out_h == 0 {
        return Err(ExportError::Probe(format!("vídeo pequeno demais: {w}x{h}")));
    }
    let total = ((end - start) * u64::from(opts.fps)).div_ceil(1000);

    let (dec_cmd, enc_cmd) = build_commands(opts, &video, out, &info, (start, end), total);

    let mut dec = Proc::spawn(&opts.ffmpeg_bin, dec_cmd, Stdio::null(), Stdio::piped())?;
    let mut enc = match Proc::spawn(&opts.ffmpeg_bin, enc_cmd, Stdio::piped(), Stdio::null()) {
        Ok(e) => e,
        Err(e) => {
            dec.kill();
            return Err(e);
        }
    };

    let result = pump(
        &mut dec,
        &mut enc,
        tl,
        start,
        total,
        (w, h),
        opts.fps,
        progress,
    );
    let result = match result {
        Ok(frames) => {
            drop(enc.child.stdin.take()); // EOF para o codificador
            enc.finish("encode")
                .and_then(|()| dec.finish("decode"))
                .map(|()| frames)
        }
        Err(e) => {
            dec.kill();
            enc.kill();
            Err(e)
        }
    };
    match result {
        Ok(frames) => Ok(ExportReport {
            frames,
            out_duration_ms: (frames * 1000 + u64::from(opts.fps) / 2) / u64::from(opts.fps),
            out_width: out_w,
            out_height: out_h,
        }),
        Err(e) => {
            let _ = fs::remove_file(out);
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pump(
    dec: &mut Proc,
    enc: &mut Proc,
    tl: &Timeline,
    start: u64,
    total: u64,
    (w, h): (u32, u32),
    fps: u32,
    progress: &mut dyn FnMut(f32),
) -> Result<u64, ExportError> {
    let (Some(mut rd), Some(mut wr)) = (dec.child.stdout.take(), enc.child.stdin.take()) else {
        return Err(ExportError::Io(io::Error::other("pipes indisponíveis")));
    };
    let len = w as usize * h as usize * 3;
    let (mut src, mut dst) = (vec![0u8; len], vec![0u8; len]);
    let mut frames = 0;
    while frames < total {
        match rd.read_exact(&mut src) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        let rect = frame_rect(tl, start, frames, fps, w, h);
        render_frame(&src, w, h, &rect, &mut dst);
        if let Err(e) = wr.write_all(&dst) {
            let stderr = enc.stderr();
            return Err(ExportError::Ffmpeg {
                stage: "encode",
                status: format!("pipe: {e}"),
                stderr,
            });
        }
        frames += 1;
        progress(frames as f32 / total as f32);
    }
    drop(rd);
    if frames == 0 {
        // Decodificador não entregou nada: devolve o motivo dele.
        let _ = dec.child.wait();
        return Err(ExportError::Ffmpeg {
            stage: "decode",
            status: "nenhum frame decodificado".into(),
            stderr: dec.stderr(),
        });
    }
    enc.child.stdin = Some(wr);
    Ok(frames)
}

#[cfg(test)]
mod tests;
