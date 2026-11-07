//! Inspeção de mídia via `ffprobe` (mesmo diretório do `ffmpeg` informado).
use crate::{ExportError, run_capture};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_ms: u64,
    pub has_audio: bool,
}

/// `ffmpeg` -> `ffprobe`, `/x/ffmpeg.exe` -> `/x/ffprobe.exe`; outros nomes usam `ffprobe` do PATH.
fn ffprobe_path(ffmpeg: &Path) -> PathBuf {
    match ffmpeg
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("ffmpeg"))
    {
        Some(rest) => ffmpeg.with_file_name(format!("ffprobe{rest}")),
        None => PathBuf::from("ffprobe"),
    }
}

fn rate(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/')?;
    let (n, d) = (n.parse::<f64>().ok()?, d.parse::<f64>().ok()?);
    (d > 0.0 && n > 0.0).then(|| n / d)
}

fn secs_ms(v: &Value) -> Option<u64> {
    let s: f64 = v.as_str()?.parse().ok()?;
    (s.is_finite() && s >= 0.0).then(|| (s * 1000.0).round() as u64)
}

/// # Errors
/// ffprobe ausente/falho ou saída sem fluxo de vídeo utilizável.
pub fn probe(ffmpeg_bin: &Path, video: &Path) -> Result<ProbeInfo, ExportError> {
    let bin = ffprobe_path(ffmpeg_bin);
    let mut cmd = Command::new(&bin);
    cmd.args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_streams",
        "-show_format",
    ])
    .arg(video);
    let out = run_capture(&bin, &mut cmd)?;
    let bad = |m: &str| ExportError::Probe(format!("{}: {m}", video.display()));
    let json: Value = serde_json::from_slice(&out).map_err(|e| bad(&format!("json: {e}")))?;
    let streams = json["streams"].as_array().map_or(&[][..], Vec::as_slice);
    let is = |s: &Value, kind: &str| s["codec_type"] == kind;
    let v = streams
        .iter()
        .find(|s| is(s, "video") && s["disposition"]["attached_pic"] != 1)
        .ok_or_else(|| bad("sem fluxo de vídeo"))?;
    let dim = |k: &str| v[k].as_u64().and_then(|n| u32::try_from(n).ok());
    let (width, height) = dim("width")
        .zip(dim("height"))
        .filter(|&(w, h)| w > 0 && h > 0)
        .ok_or_else(|| bad("dimensões ausentes"))?;
    let fps = ["avg_frame_rate", "r_frame_rate"]
        .iter()
        .find_map(|k| v[*k].as_str().and_then(rate))
        .ok_or_else(|| bad("fps ausente"))?;
    let duration_ms = secs_ms(&json["format"]["duration"])
        .or_else(|| secs_ms(&v["duration"]))
        .ok_or_else(|| bad("duração ausente"))?;
    Ok(ProbeInfo {
        width,
        height,
        fps,
        duration_ms,
        has_audio: streams.iter().any(|s| is(s, "audio")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffprobe_name_derivation() {
        assert_eq!(ffprobe_path(Path::new("ffmpeg")), Path::new("ffprobe"));
        assert_eq!(
            ffprobe_path(Path::new("/opt/bin/ffmpeg.exe")),
            Path::new("/opt/bin/ffprobe.exe")
        );
        assert_eq!(ffprobe_path(Path::new("/x/custom")), Path::new("ffprobe"));
    }

    #[test]
    fn parses_rates() {
        assert_eq!(rate("30/1"), Some(30.0));
        assert_eq!(rate("0/0"), None);
        assert_eq!(rate("x"), None);
    }
}
