//! Captura de tela + eventos de mouse. Contrato estável; backends em módulos próprios.
//! `unsafe` permitido SOMENTE em `windows.rs`.
#![cfg_attr(not(windows), forbid(unsafe_code))]

use std::path::{Path, PathBuf};
use thiserror::Error;
use zoomcast_core::events::EventLog;

pub mod args;
pub mod coords;
pub mod dshow;
pub mod synthetic;
#[cfg(windows)]
pub mod windows;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureTarget {
    PrimaryMonitor,
    Monitor(u32),
    /// Título exato da janela.
    Window(String),
}

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub target: CaptureTarget,
    pub fps: u32,
    /// Raiz do pacote `.zoomcast`; o vídeo vai para `<out_dir>/media/screen.mp4`.
    pub out_dir: PathBuf,
    /// Nome do dispositivo dshow; `None` = sem áudio.
    pub mic: Option<String>,
    pub ffmpeg_bin: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CaptureResult {
    /// Relativo a `out_dir` (ex.: `media/screen.mp4`).
    pub video_path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_ms: u64,
    pub has_audio: bool,
    /// Atraso medido (ms) entre t0 dos eventos e o 1º frame do vídeo; já aplicado em `events`.
    pub start_offset_ms: i64,
    pub events: EventLog,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("ffmpeg não encontrado ({0}); defina ZOOMCAST_FFMPEG")]
    FfmpegNotFound(String),
    #[error("região/alvo inválido: {0}")]
    InvalidTarget(String),
    #[error("captura já iniciada")]
    AlreadyStarted,
    #[error("captura não iniciada")]
    NotStarted,
    #[error("ffmpeg falhou: {0}")]
    Ffmpeg(String),
    #[error("backend indisponível nesta plataforma: {0}")]
    Unsupported(&'static str),
    #[error("e/s: {0}")]
    Io(#[from] std::io::Error),
}

pub trait CaptureBackend {
    /// # Errors
    /// Alvo inválido, ffmpeg ausente ou já iniciada.
    fn start(&mut self, cfg: &CaptureConfig) -> Result<(), CaptureError>;
    /// Para, finaliza o MP4 e devolve metadados + eventos.
    /// # Errors
    /// Não iniciada ou falha do ffmpeg.
    fn stop(&mut self) -> Result<CaptureResult, CaptureError>;
}

#[must_use]
pub fn default_backend() -> Box<dyn CaptureBackend> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsBackend::default())
    }
    #[cfg(not(windows))]
    {
        Box::new(synthetic::SyntheticBackend::default())
    }
}

/// Lista dispositivos de áudio dshow (Windows). Outras plataformas: vazio.
/// # Errors
/// ffmpeg ausente.
pub fn list_audio_devices(ffmpeg_bin: &Path) -> Result<Vec<String>, CaptureError> {
    #[cfg(windows)]
    {
        let out = std::process::Command::new(ffmpeg_bin)
            .args([
                "-hide_banner",
                "-list_devices",
                "true",
                "-f",
                "dshow",
                "-i",
                "dummy",
            ])
            .output()
            .map_err(|e| CaptureError::FfmpegNotFound(format!("{}: {e}", ffmpeg_bin.display())))?;
        Ok(dshow::parse_audio_devices(&String::from_utf8_lossy(
            &out.stderr,
        )))
    }
    #[cfg(not(windows))]
    {
        let _ = ffmpeg_bin;
        Ok(Vec::new())
    }
}
