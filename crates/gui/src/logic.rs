//! Lógica pura (sem UI/threads): validação de entradas e montagem de configs.
use std::path::{Path, PathBuf};
use zoomcast_capture::{CaptureConfig, CaptureResult, CaptureTarget};
use zoomcast_core::{
    project::{FORMAT_VERSION, Manifest, MediaInfo},
    timeline::{MAX_SCALE, MIN_SCALE, Timeline},
    zoomgen::{self, ZoomRule},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Primary,
    Monitor,
    Window,
}

#[derive(Debug, Clone)]
pub struct Form {
    pub dir: String,
    pub kind: TargetKind,
    /// Texto do campo "monitor N".
    pub monitor: String,
    /// Título exato da janela.
    pub window: String,
    pub zoom: f64,
    /// `None` = sem áudio.
    pub mic: Option<String>,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            dir: String::new(),
            kind: TargetKind::Primary,
            monitor: "0".into(),
            window: String::new(),
            zoom: ZoomRule::default().scale,
            mic: None,
        }
    }
}

/// Pasta padrão quando o campo está vazio: `<Vídeos>/Zoomcast/rec-<unix_secs>`.
pub fn auto_dir(unix_secs: u64) -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(std::env::temp_dir, PathBuf::from);
    home.join("Videos")
        .join("Zoomcast")
        .join(format!("rec-{unix_secs}"))
}

/// `FFmpeg`: `ZOOMCAST_FFMPEG`, senão `ffmpeg(.exe)` ao lado do executável, senão `PATH`.
pub fn ffmpeg_bin() -> PathBuf {
    if let Some(v) = std::env::var_os("ZOOMCAST_FFMPEG").filter(|v| !v.is_empty()) {
        return v.into();
    }
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

/// Pasta do projeto: obrigatória (sem espaços nas pontas).
pub fn parse_dir(s: &str) -> Result<PathBuf, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("informe a pasta do projeto".into());
    }
    Ok(PathBuf::from(s))
}

pub fn parse_target(f: &Form) -> Result<CaptureTarget, String> {
    match f.kind {
        TargetKind::Primary => Ok(CaptureTarget::PrimaryMonitor),
        TargetKind::Monitor => f
            .monitor
            .trim()
            .parse::<u32>()
            .map(CaptureTarget::Monitor)
            .map_err(|_| "monitor deve ser inteiro >= 0".into()),
        TargetKind::Window if f.window.trim().is_empty() => {
            Err("informe o título da janela".into())
        }
        // Título exato: não aparar espaços.
        TargetKind::Window => Ok(CaptureTarget::Window(f.window.clone())),
    }
}

pub fn check_zoom(z: f64) -> Result<f64, String> {
    if z.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(&z) {
        Ok(z)
    } else {
        Err("zoom deve estar entre 1 e 4".into())
    }
}

/// Nunca sobrescrever mídia capturada (mesma regra do CLI).
pub fn check_dir_free(root: &Path) -> Result<(), String> {
    if root.join("manifest.json").exists() || root.join("media/screen.mp4").exists() {
        return Err("projeto já contém captura; escolha outra pasta".into());
    }
    Ok(())
}

/// Monta `CaptureConfig` + zoom validados. Sem tocar o disco.
pub fn build_config(f: &Form, ffmpeg_bin: PathBuf) -> Result<(CaptureConfig, f64), String> {
    let out_dir = parse_dir(&f.dir)?;
    let target = parse_target(f)?;
    let zoom = check_zoom(f.zoom)?;
    let mic = f.mic.clone().filter(|m| !m.trim().is_empty());
    Ok((
        CaptureConfig {
            target,
            fps: 30,
            out_dir,
            mic,
            ffmpeg_bin,
        },
        zoom,
    ))
}

/// Manifest a partir da captura (mesmo fluxo do CLI).
pub fn build_manifest(
    c: &CaptureResult,
    zoom: f64,
    created_unix_ms: u64,
) -> Result<Manifest, String> {
    let video_path = c
        .video_path
        .to_str()
        .ok_or("caminho de vídeo inválido")?
        .replace('\\', "/");
    let zooms = zoomgen::generate(
        &c.events,
        c.width,
        c.height,
        c.duration_ms,
        ZoomRule {
            scale: zoom,
            ..ZoomRule::default()
        },
    );
    Ok(Manifest {
        format_version: FORMAT_VERSION,
        created_unix_ms,
        media: MediaInfo {
            video_path,
            width: c.width,
            height: c.height,
            fps: c.fps,
            duration_ms: c.duration_ms,
            has_audio: c.has_audio,
        },
        events_path: "events.json".into(),
        timeline: Timeline {
            zooms,
            ..Timeline::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zoomcast_core::events::EventLog;

    fn form() -> Form {
        Form {
            dir: " C:/p/x ".into(),
            ..Form::default()
        }
    }

    #[test]
    fn auto_dir_has_stamp() {
        assert!(auto_dir(42).ends_with("Zoomcast/rec-42"));
    }

    #[test]
    fn default_is_primary_no_mic() {
        let (c, z) = build_config(&form(), "ffmpeg".into()).unwrap();
        assert_eq!(c.target, CaptureTarget::PrimaryMonitor);
        assert_eq!(c.out_dir, PathBuf::from("C:/p/x"));
        assert_eq!((c.fps, c.mic, z), (30, None, 2.0));
    }

    #[test]
    fn monitor_and_window_targets() {
        let f = Form {
            kind: TargetKind::Monitor,
            monitor: " 2 ".into(),
            ..form()
        };
        assert_eq!(parse_target(&f).unwrap(), CaptureTarget::Monitor(2));
        let f = Form {
            kind: TargetKind::Window,
            window: " Bloco ".into(),
            ..form()
        };
        assert_eq!(
            parse_target(&f).unwrap(),
            CaptureTarget::Window(" Bloco ".into())
        );
    }

    #[test]
    fn rejects_bad_input() {
        let bad_dir = Form {
            dir: "  ".into(),
            ..form()
        };
        assert!(build_config(&bad_dir, "f".into()).is_err());
        for m in ["", "-1", "x", "1.5"] {
            let f = Form {
                kind: TargetKind::Monitor,
                monitor: m.into(),
                ..form()
            };
            assert!(parse_target(&f).is_err(), "{m}");
        }
        let f = Form {
            kind: TargetKind::Window,
            window: "  ".into(),
            ..form()
        };
        assert!(parse_target(&f).is_err());
        for z in [0.9, 4.1, f64::NAN, f64::INFINITY] {
            assert!(check_zoom(z).is_err(), "{z}");
        }
        assert_eq!(check_zoom(1.0), Ok(1.0));
        assert_eq!(check_zoom(4.0), Ok(4.0));
    }

    #[test]
    fn mic_passthrough_and_blank_is_none() {
        let f = Form {
            mic: Some("Mic USB".into()),
            ..form()
        };
        assert_eq!(
            build_config(&f, "f".into()).unwrap().0.mic.as_deref(),
            Some("Mic USB")
        );
        let f = Form {
            mic: Some(" ".into()),
            ..form()
        };
        assert_eq!(build_config(&f, "f".into()).unwrap().0.mic, None);
    }

    #[test]
    fn dir_free_checks_existing_capture() {
        let d = std::env::temp_dir().join(format!("zc-gui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert!(check_dir_free(&d).is_ok());
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("manifest.json"), "{}").unwrap();
        assert!(check_dir_free(&d).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn manifest_uses_zoom_and_forward_slashes() {
        let c = CaptureResult {
            video_path: PathBuf::from("media\\screen.mp4"),
            width: 1920,
            height: 1080,
            fps: 30.0,
            duration_ms: 5000,
            has_audio: true,
            start_offset_ms: 0,
            events: EventLog::default(),
        };
        let m = build_manifest(&c, 3.0, 7).unwrap();
        assert_eq!(m.media.video_path, "media/screen.mp4");
        assert_eq!((m.created_unix_ms, m.media.has_audio), (7, true));
        assert_eq!(m.timeline.zooms, Vec::new()); // sem cliques
    }
}
