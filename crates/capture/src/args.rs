//! Argumentos do ffmpeg (gdigrab + dshow) e parser do `-progress`. Puro.

use crate::CaptureError;
use crate::coords::Region;
use std::ffi::OsString;
use std::path::Path;

/// Caminho do vídeo relativo a `out_dir`.
pub const VIDEO_REL: &str = "media/screen.mp4";

pub const MAX_FPS: u32 = 240;

/// Linha de `-progress pipe:1` que nos interessa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Quadros já escritos no muxer.
    Frame(u64),
    /// Duração de mídia escrita (µs). (`out_time_ms` do ffmpeg também é µs; usamos `out_time_us`.)
    OutTimeUs(u64),
}

#[must_use]
pub fn parse_progress_line(line: &str) -> Option<Progress> {
    let line = line.trim();
    if let Some(v) = line.strip_prefix("frame=") {
        return v.trim().parse().ok().map(Progress::Frame);
    }
    line.strip_prefix("out_time_us=")?
        .trim()
        .parse()
        .ok()
        .map(Progress::OutTimeUs)
}

/// Captura válida mesmo se o ffmpeg sair com código != 0 (ex.: 0xc000013a no Windows ao
/// receber `q`): exige ao menos 1 frame escrito e arquivo de saída não vazio.
#[must_use]
pub fn capture_ok(exit_success: bool, last_frame: u64, file_len: Option<u64>) -> bool {
    last_frame > 0 && (exit_success || file_len.is_some_and(|n| n > 0))
}

/// Monta os argumentos do ffmpeg. Vídeo: desktop (gdigrab) recortado em `region`.
/// Relógio: `use_wallclock_as_timestamps` nas entradas + `cfr` na saída → PTS = tempo real.
///
/// # Errors
/// `InvalidTarget` se fps ∉ 1..=240, mic vazio, região ímpar/< 2x2.
pub fn build_ffmpeg_args(
    fps: u32,
    region: Region,
    mic: Option<&str>,
    out: &Path,
) -> Result<Vec<OsString>, CaptureError> {
    if !(1..=MAX_FPS).contains(&fps) {
        return Err(CaptureError::InvalidTarget(format!(
            "fps {fps} fora de 1..={MAX_FPS}"
        )));
    }
    if region.w < 2 || region.h < 2 || region.w % 2 != 0 || region.h % 2 != 0 {
        return Err(CaptureError::InvalidTarget(format!(
            "região {}x{} inválida (par, ≥ 2x2)",
            region.w, region.h
        )));
    }
    if mic.is_some_and(|m| m.trim().is_empty()) {
        return Err(CaptureError::InvalidTarget(
            "nome do microfone vazio".into(),
        ));
    }
    let mut a: Vec<OsString> = Vec::new();
    let mut push = |s: &str| a.push(s.into());
    for s in [
        "-hide_banner",
        "-loglevel",
        "warning",
        "-nostats",
        "-progress",
        "pipe:1",
        "-stats_period",
        "0.05",
        "-y",
    ] {
        push(s);
    }
    // Vídeo
    for s in [
        "-f",
        "gdigrab",
        "-use_wallclock_as_timestamps",
        "1",
        "-probesize",
        "32M",
        "-analyzeduration",
        "0",
        "-framerate",
    ] {
        push(s);
    }
    push(&fps.to_string());
    push("-offset_x");
    push(&region.x.to_string());
    push("-offset_y");
    push(&region.y.to_string());
    push("-video_size");
    push(&format!("{}x{}", region.w, region.h));
    push("-i");
    push("desktop");
    // Áudio
    if let Some(m) = mic {
        for s in ["-f", "dshow", "-use_wallclock_as_timestamps", "1", "-i"] {
            push(s);
        }
        push(&format!("audio={m}"));
        for s in [
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:a",
            "aac",
            "-b:a",
            "160k",
            "-af",
            "aresample=async=1:first_pts=0",
        ] {
            push(s);
        }
    }
    // Codec de vídeo + saída
    for s in [
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-tune",
        "zerolatency",
        "-crf",
        "18",
        "-pix_fmt",
        "yuv420p",
        "-fps_mode",
        "cfr",
        // MP4 fragmentado: legível mesmo se o ffmpeg for encerrado sem finalizar o moov.
        "-movflags",
        "frag_keyframe+empty_moov",
        "-r",
    ] {
        push(s);
    }
    push(&fps.to_string());
    a.push(out.as_os_str().to_owned());
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Region = Region {
        x: -1920,
        y: 10,
        w: 1280,
        h: 720,
    };

    fn s(v: &[OsString]) -> Vec<String> {
        v.iter().map(|x| x.to_string_lossy().into_owned()).collect()
    }
    fn pos(v: &[String], k: &str) -> usize {
        v.iter()
            .position(|x| x == k)
            .unwrap_or_else(|| panic!("falta {k}"))
    }

    #[test]
    fn so_video() {
        let a = s(&build_ffmpeg_args(30, R, None, Path::new("o/media/screen.mp4")).unwrap());
        assert_eq!(a[pos(&a, "-offset_x") + 1], "-1920");
        assert_eq!(a[pos(&a, "-offset_y") + 1], "10");
        assert_eq!(a[pos(&a, "-video_size") + 1], "1280x720");
        assert_eq!(a[pos(&a, "-framerate") + 1], "30");
        assert_eq!(a.last().unwrap(), "o/media/screen.mp4");
        assert!(!a.contains(&"dshow".to_string()));
        assert_eq!(a.iter().filter(|x| *x == "-i").count(), 1);
    }

    #[test]
    fn com_audio_nome_com_espacos_e_aspas_parenteses() {
        let a = s(&build_ffmpeg_args(
            60,
            R,
            Some("Microfone (Realtek(R) Audio)"),
            Path::new("x.mp4"),
        )
        .unwrap());
        // um único argumento (sem shell, sem escape)
        assert!(a.contains(&"audio=Microfone (Realtek(R) Audio)".to_string()));
        assert_eq!(a.iter().filter(|x| *x == "-i").count(), 2);
        assert!(a.contains(&"aac".to_string()));
    }

    #[test]
    fn invalidos() {
        let p = Path::new("x.mp4");
        assert!(build_ffmpeg_args(0, R, None, p).is_err());
        assert!(build_ffmpeg_args(241, R, None, p).is_err());
        assert!(build_ffmpeg_args(30, Region { w: 1281, ..R }, None, p).is_err());
        assert!(build_ffmpeg_args(30, R, Some("  "), p).is_err());
    }

    #[test]
    fn capture_ok_tolera_exit_code_com_arquivo() {
        assert!(capture_ok(true, 5, None));
        assert!(capture_ok(false, 5, Some(1000))); // 0xc000013a com vídeo gravado
        assert!(!capture_ok(false, 5, Some(0)));
        assert!(!capture_ok(false, 5, None));
        assert!(!capture_ok(true, 0, Some(1000)));
    }

    #[test]
    fn mp4_fragmentado() {
        let a = s(&build_ffmpeg_args(30, R, None, Path::new("x.mp4")).unwrap());
        assert_eq!(a[pos(&a, "-movflags") + 1], "frag_keyframe+empty_moov");
    }

    #[test]
    fn progress() {
        assert_eq!(parse_progress_line("frame=12"), Some(Progress::Frame(12)));
        assert_eq!(
            parse_progress_line("out_time_us=1500000\n"),
            Some(Progress::OutTimeUs(1_500_000))
        );
        assert_eq!(parse_progress_line("out_time_us=N/A"), None);
        assert_eq!(parse_progress_line("fps=30.0"), None);
        assert_eq!(parse_progress_line("progress=end"), None);
    }
}
