use super::*;
use zoomcast_core::{
    camera::camera_at_f,
    events::EventLog,
    project::{FORMAT_VERSION, Manifest, MediaInfo},
    timeline::ZoomSegment,
};

fn ffmpeg_ok() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok()
        && Command::new("ffprobe").arg("-version").output().is_ok()
}

macro_rules! need_ffmpeg {
    () => {
        if !ffmpeg_ok() {
            eprintln!("skip: ffmpeg/ffprobe ausente");
            return;
        }
    };
}

fn ffmpeg(args: &[&str]) {
    let o = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .output()
        .expect("ffmpeg");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

fn zoom() -> ZoomSegment {
    ZoomSegment {
        id: 1,
        start_ms: 1000,
        end_ms: 3000,
        scale: 2.0,
        center_x: 0.5,
        center_y: 0.5,
        ramp_ms: 400,
        enabled: true,
    }
}

/// Projeto 640x360@30, 4s, com tom de áudio opcional; trim 500..3500, zoom 2x em 1000..3000.
fn make_project(dir: &Path, audio: bool, size: &str) -> Project {
    let root = dir.join("t.zoomcast");
    fs::create_dir_all(root.join("media")).unwrap();
    let vid = root.join("media/screen.mp4");
    let src = format!(
        "{}=size={size}:rate=30",
        if size == "321x181" {
            "testsrc"
        } else {
            "testsrc2"
        }
    );
    let mut a = vec!["-f", "lavfi", "-i", &src];
    if audio {
        a.extend(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"]);
    }
    let pix = if size == "321x181" {
        "yuv444p"
    } else {
        "yuv420p"
    };
    a.extend(["-t", "4", "-c:v", "libx264", "-crf", "12", "-pix_fmt", pix]);
    if audio {
        a.extend(["-c:a", "aac"]);
    }
    a.push(vid.to_str().unwrap());
    ffmpeg(&a);
    let info = probe(Path::new("ffmpeg"), &vid).unwrap();
    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        created_unix_ms: 1,
        media: MediaInfo {
            video_path: "media/screen.mp4".into(),
            width: info.width,
            height: info.height,
            fps: info.fps,
            duration_ms: info.duration_ms,
            has_audio: info.has_audio,
        },
        events_path: "events.json".into(),
        timeline: Timeline {
            trim_start_ms: 500,
            trim_end_ms: Some(3500),
            zooms: vec![zoom()],
        },
    };
    Project::create(&root, manifest, &EventLog::default()).unwrap()
}

fn frame_at(video: &Path, t_s: f64) -> Vec<u8> {
    let o = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-ss"])
        .arg(format!("{t_s}"))
        .arg("-i")
        .arg(video)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("ffmpeg");
    assert!(o.status.success());
    o.stdout
}

fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let s: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    s as f64 / a.len() as f64
}

#[test]
fn rect_matches_camera_at_f() {
    let tl = Timeline {
        trim_start_ms: 500,
        trim_end_ms: Some(3500),
        zooms: vec![ZoomSegment {
            center_x: 0.9,
            center_y: 0.2,
            ..zoom()
        }],
    };
    let (w, h, fps) = (640u32, 360u32, 30u32);
    let mut zoomed = 0;
    for k in 0..90u32 {
        let t = 500.0 + f64::from(k) * 1000.0 / f64::from(fps);
        let cam = camera_at_f(&tl, t);
        let r = frame_rect(&tl, 500, u64::from(k), fps, w, h);
        let (fw, fh) = (f64::from(w), f64::from(h));
        let (ew, eh) = (fw / cam.scale, fh / cam.scale);
        let (ex, ey) = (cam.center_x * fw - ew / 2.0, cam.center_y * fh - eh / 2.0);
        for (g, e) in [(r.x, ex), (r.y, ey), (r.w, ew), (r.h, eh)] {
            assert!((g - e).abs() < 1e-9, "k={k}: {r:?} vs cam {cam:?}");
        }
        assert!(r.x >= -1e-9 && r.x + r.w <= fw + 1e-9 && r.y >= -1e-9 && r.y + r.h <= fh + 1e-9);
        zoomed += usize::from(cam.scale > 1.0);
    }
    assert!(zoomed > 40, "zoom deveria estar ativo em muitos frames");
    // Fora do zoom: retângulo cheio.
    assert_eq!(
        frame_rect(&tl, 500, 0, fps, w, h),
        Rect {
            x: 0.0,
            y: 0.0,
            w: 640.0,
            h: 360.0
        }
    );
}

#[test]
#[allow(clippy::many_single_char_names)]
fn validates_inputs_without_ffmpeg() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("p.zoomcast");
    let m = Manifest {
        format_version: FORMAT_VERSION,
        created_unix_ms: 1,
        media: MediaInfo {
            video_path: "media/screen.mp4".into(),
            width: 64,
            height: 64,
            fps: 30.0,
            duration_ms: 1000,
            has_audio: false,
        },
        events_path: "events.json".into(),
        timeline: Timeline::default(),
    };
    let p = Project::create(&root, m, &EventLog::default()).unwrap();
    let mut noop = |_: f32| {};
    let mut o = ExportOptions::default();
    // saída == entrada
    let same = p.video_file();
    fs::write(&same, b"x").unwrap();
    let e = export_project(&p, &same, &o, &mut noop).unwrap_err();
    assert!(matches!(e, ExportError::SameInputOutput(_)), "{e}");
    // fps / crf inválidos
    o.fps = 0;
    assert!(matches!(
        export_project(&p, &d.path().join("o.mp4"), &o, &mut noop),
        Err(ExportError::InvalidOptions(_))
    ));
    o.fps = 30;
    o.crf = 99;
    assert!(matches!(
        export_project(&p, &d.path().join("o.mp4"), &o, &mut noop),
        Err(ExportError::InvalidOptions(_))
    ));
    // ffmpeg ausente
    o.crf = 20;
    o.ffmpeg_bin = "/nao/existe/ffmpeg".into();
    let e = export_project(&p, &d.path().join("o.mp4"), &o, &mut noop).unwrap_err();
    assert!(matches!(e, ExportError::FfmpegNotFound(_)), "{e}");
    assert!(e.to_string().contains("ZOOMCAST_FFMPEG"));
}

#[test]
fn empty_trim_is_error() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let mut p = make_project(d.path(), false, "320x180");
    p.manifest.timeline.trim_start_ms = 2000;
    p.manifest.timeline.trim_end_ms = Some(2000);
    let out = d.path().join("o.mp4");
    let e = export_project(&p, &out, &ExportOptions::default(), &mut |_| {}).unwrap_err();
    assert!(matches!(e, ExportError::Core(_)), "{e}");
    assert!(!out.exists());
}

#[test]
fn export_end_to_end() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let p = make_project(d.path(), true, "640x360");
    let out = d.path().join("out.mp4");
    let mut last = 0.0;
    let mut calls = 0;
    let rep = export_project(&p, &out, &ExportOptions::default(), &mut |f| {
        assert!(f >= last && f <= 1.0);
        last = f;
        calls += 1;
    })
    .unwrap();
    assert_eq!((rep.out_width, rep.out_height), (640, 360));
    assert_eq!(rep.frames, 90);
    assert_eq!(calls, 90);
    assert!((last - 1.0).abs() < 1e-6);

    let info = probe(Path::new("ffmpeg"), &out).unwrap();
    assert_eq!((info.width, info.height), (640, 360));
    assert!(info.has_audio, "áudio deveria ser preservado");
    assert!(
        info.duration_ms.abs_diff(3000) <= 34,
        "duração {}",
        info.duration_ms
    );
    assert!((info.fps - 30.0).abs() < 0.01);

    // Sem zoom: saída em t=100ms == fonte em 600ms (frame exato; só perda do x264).
    let (o_in, s_in) = (frame_at(&out, 0.1), frame_at(&p.video_file(), 0.6));
    let d_in = mean_abs_diff(&o_in, &s_in);
    assert!(d_in < 2.0, "fora do zoom deve ser ~idêntico, diff {d_in}");

    // Meio do zoom: saída em 1500ms == fonte em 2000ms com recorte 2x central.
    let (o_z, s_z) = (frame_at(&out, 1.5), frame_at(&p.video_file(), 2.0));
    let d_raw = mean_abs_diff(&o_z, &s_z);
    assert!(
        d_raw > 10.0,
        "frame com zoom deve diferir do original: {d_raw}"
    );
    let mut expect = vec![0; s_z.len()];
    let rect = frame_rect(&p.manifest.timeline, 500, 45, 30, 640, 360);
    assert!((rect.w - 320.0).abs() < 1e-9);
    render_frame(&s_z, 640, 360, &rect, &mut expect);
    let d_exp = mean_abs_diff(&o_z, &expect);
    assert!(
        d_exp < 4.0,
        "frame com zoom deve bater com o recorte esperado: {d_exp}"
    );
}

#[test]
fn odd_dimensions_become_even_and_no_audio() {
    need_ffmpeg!();
    let d = tempfile::tempdir().unwrap();
    let p = make_project(d.path(), false, "321x181");
    assert_eq!(
        (p.manifest.media.width, p.manifest.media.height),
        (321, 181)
    );
    let out = d.path().join("odd.mp4");
    let rep = export_project(&p, &out, &ExportOptions::default(), &mut |_| {}).unwrap();
    let info = probe(Path::new("ffmpeg"), &out).unwrap();
    assert_eq!((info.width % 2, info.height % 2), (0, 0));
    assert_eq!((info.width, info.height), (rep.out_width, rep.out_height));
    assert!(!info.has_audio);
}
