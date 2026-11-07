//! Backend sintético (não-Windows / testes): vídeo `testsrc2` via ffmpeg + eventos determinísticos.
use crate::{CaptureBackend, CaptureConfig, CaptureError, CaptureResult};
use std::{
    io::ErrorKind,
    path::PathBuf,
    process::{Command, Stdio},
    time::Instant,
};
use zoomcast_core::events::{EventLog, InputEvent, MouseButton};

const W: u32 = 640;
const H: u32 = 360;
const FPS: u32 = 30;
const MIN_MS: u64 = 2000;
const CLICKS_MS: [u64; 2] = [1500, 3500];
const MOVE_STEP_MS: u64 = 100;

#[derive(Debug, Default)]
pub struct SyntheticBackend {
    started: Option<(Instant, CaptureConfig)>,
}

/// Posição determinística do cursor em `t_ms` (elipse dentro do vídeo).
/// Arredondada a 0,01 px: roundtrip exato em JSON.
fn pos(t_ms: u64) -> (f64, f64) {
    let a = t_ms as f64 / 1000.0;
    let q = |v: f64| (v * 100.0).round() / 100.0;
    (
        q(f64::from(W) / 2.0 + 200.0 * a.sin()),
        q(f64::from(H) / 2.0 + 100.0 * (a * 1.3).cos()),
    )
}

fn events(duration_ms: u64) -> EventLog {
    let mut events: Vec<InputEvent> = (0..duration_ms)
        .step_by(MOVE_STEP_MS as usize)
        .map(|t_ms| {
            let (x, y) = pos(t_ms);
            InputEvent::Move { t_ms, x, y }
        })
        .collect();
    events.extend(CLICKS_MS.iter().filter(|&&t| t < duration_ms).map(|&t_ms| {
        let (x, y) = pos(t_ms);
        InputEvent::Click {
            t_ms,
            x,
            y,
            button: MouseButton::Left,
        }
    }));
    let mut log = EventLog { events };
    log.sort();
    log
}

impl CaptureBackend for SyntheticBackend {
    fn start(&mut self, cfg: &CaptureConfig) -> Result<(), CaptureError> {
        if self.started.is_some() {
            return Err(CaptureError::AlreadyStarted);
        }
        self.started = Some((Instant::now(), cfg.clone()));
        Ok(())
    }

    fn stop(&mut self) -> Result<CaptureResult, CaptureError> {
        let (t0, cfg) = self.started.take().ok_or(CaptureError::NotStarted)?;
        let want_ms = (t0.elapsed().as_millis() as u64).max(MIN_MS);
        let frames = (want_ms * u64::from(FPS)).div_ceil(1000);
        let duration_ms = frames * 1000 / u64::from(FPS);

        let video_path = PathBuf::from("media").join("screen.mp4");
        std::fs::create_dir_all(cfg.out_dir.join("media"))?;
        let out = Command::new(&cfg.ffmpeg_bin)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg(format!("testsrc2=size={W}x{H}:rate={FPS}"))
            .args(["-frames:v", &frames.to_string()])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-an"])
            .arg(cfg.out_dir.join(&video_path))
            .stdin(Stdio::null())
            .output()
            .map_err(|e| {
                if e.kind() == ErrorKind::NotFound {
                    CaptureError::FfmpegNotFound(cfg.ffmpeg_bin.display().to_string())
                } else {
                    CaptureError::Io(e)
                }
            })?;
        if !out.status.success() {
            return Err(CaptureError::Ffmpeg(
                String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            ));
        }
        Ok(CaptureResult {
            video_path,
            width: W,
            height: H,
            fps: f64::from(FPS),
            duration_ms,
            has_audio: false,
            start_offset_ms: 0,
            events: events(duration_ms),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CaptureTarget;
    use std::path::Path;
    use zoomcast_core::project::{FORMAT_VERSION, Manifest, MediaInfo, Project};
    use zoomcast_core::timeline::Timeline;

    fn cfg(dir: &Path) -> CaptureConfig {
        CaptureConfig {
            target: CaptureTarget::PrimaryMonitor,
            fps: 30,
            out_dir: dir.to_owned(),
            mic: None,
            ffmpeg_bin: "ffmpeg".into(),
        }
    }

    fn probe(file: &Path, entries: &str, extra: &[&str]) -> String {
        let o = Command::new("ffprobe")
            .args(["-v", "error"])
            .args(extra)
            .args(["-show_entries", entries, "-of", "csv=p=0"])
            .arg(file)
            .output()
            .expect("ffprobe");
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn stop_without_start() {
        assert!(matches!(
            SyntheticBackend::default().stop(),
            Err(CaptureError::NotStarted)
        ));
    }

    #[test]
    fn double_start() {
        let d = tempfile::tempdir().unwrap();
        let mut b = SyntheticBackend::default();
        b.start(&cfg(d.path())).unwrap();
        assert!(matches!(
            b.start(&cfg(d.path())),
            Err(CaptureError::AlreadyStarted)
        ));
    }

    #[test]
    fn missing_ffmpeg() {
        let d = tempfile::tempdir().unwrap();
        let mut c = cfg(d.path());
        c.ffmpeg_bin = "ffmpeg-inexistente-zoomcast".into();
        let mut b = SyntheticBackend::default();
        b.start(&c).unwrap();
        assert!(matches!(b.stop(), Err(CaptureError::FfmpegNotFound(_))));
    }

    #[test]
    fn end_to_end_ffprobe_events_and_project_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("t.zoomcast");
        let mut backend = SyntheticBackend::default();
        backend.start(&cfg(&root)).unwrap();
        let r = backend.stop().unwrap();

        // metadados
        assert_eq!((r.width, r.height), (640, 360));
        assert!((r.fps - 30.0).abs() < f64::EPSILON);
        assert!(!r.has_audio);
        assert_eq!(r.start_offset_ms, 0);
        assert!(r.duration_ms >= 2000);
        assert_eq!(r.video_path, Path::new("media").join("screen.mp4"));

        // ffprobe confirma vídeo
        let file = root.join(&r.video_path);
        assert_eq!(
            probe(&file, "stream=codec_type,width,height,r_frame_rate", &[]),
            "video,640,360,30/1"
        );
        let dur: f64 = probe(&file, "format=duration", &[]).parse().unwrap();
        assert!(
            dur >= 2.0 && (dur * 1000.0 - r.duration_ms as f64).abs() < 100.0,
            "{dur}"
        );
        assert!(!probe(&file, "stream=codec_type", &["-select_streams", "a"]).contains("audio"));

        // eventos
        let ev = &r.events.events;
        assert!(ev.windows(2).all(|w| w[0].t_ms() <= w[1].t_ms()));
        assert!(ev.iter().all(|e| e.t_ms() < r.duration_ms));
        let clicks: Vec<u64> = ev
            .iter()
            .filter(|e| matches!(e, InputEvent::Click { .. }))
            .map(InputEvent::t_ms)
            .collect();
        assert_eq!(clicks, vec![1500]); // 3500 fora da duração (~2s)
        assert!(ev.iter().any(|e| matches!(e, InputEvent::Move { .. })));
        for e in ev {
            let (InputEvent::Move { x, y, .. } | InputEvent::Click { x, y, .. }) = *e;
            assert!((0.0..f64::from(W)).contains(&x) && (0.0..f64::from(H)).contains(&y));
        }
        assert_eq!(events(r.duration_ms), r.events); // determinístico

        // Project roundtrip
        let manifest = Manifest {
            format_version: FORMAT_VERSION,
            created_unix_ms: 1,
            media: MediaInfo {
                video_path: "media/screen.mp4".into(),
                width: r.width,
                height: r.height,
                fps: r.fps,
                duration_ms: r.duration_ms,
                has_audio: r.has_audio,
            },
            events_path: "events.json".into(),
            timeline: Timeline::default(),
        };
        let created = Project::create(&root, manifest, &r.events).unwrap();
        let opened = Project::open(&root).unwrap();
        assert_eq!(created.manifest, opened.manifest);
        assert_eq!(opened.read_events().unwrap(), r.events);
        assert!(opened.video_file().exists());
    }

    #[test]
    fn long_run_has_both_clicks() {
        let log = events(4000);
        let clicks: Vec<u64> = log
            .events
            .iter()
            .filter(|e| matches!(e, InputEvent::Click { .. }))
            .map(InputEvent::t_ms)
            .collect();
        assert_eq!(clicks, vec![1500, 3500]);
        assert!(log.events.windows(2).all(|w| w[0].t_ms() <= w[1].t_ms()));
    }
}
