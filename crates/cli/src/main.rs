use std::{
    env,
    error::Error,
    ffi::OsString,
    io::{self, BufRead},
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zoomcast_capture::{
    CaptureBackend, CaptureConfig, CaptureTarget, default_backend, list_audio_devices,
    synthetic::SyntheticBackend,
};
use zoomcast_core::{
    project::{FORMAT_VERSION, Manifest, MediaInfo, Project},
    timeline::{MAX_SCALE, MIN_SCALE, Timeline},
    zoomgen::{self, ZoomRule},
};
use zoomcast_export::{ExportOptions, export_project};

type CliResult<T> = Result<T, Box<dyn Error>>;

struct RecordOpts {
    /// `None` = gravar até Enter.
    seconds: Option<u64>,
    target: CaptureTarget,
    zoom: Option<f64>,
    mic: Option<String>,
}

const SECONDS_ERR: &str = "seconds deve ser inteiro entre 0 e 3600";

fn set_once<T>(slot: &mut Option<T>, v: T, flag: &str) -> CliResult<()> {
    if slot.replace(v).is_some() {
        return Err(invalid(&format!("{flag} repetido")).into());
    }
    Ok(())
}

fn parse_opts(rest: &[OsString]) -> CliResult<RecordOpts> {
    let (mut seconds, mut monitor, mut window, mut zoom, mut mic) = (None, None, None, None, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let a = a.to_str().ok_or_else(|| invalid("argumento não é UTF-8"))?;
        let mut value = |flag: &str| -> CliResult<String> {
            let v = it
                .next()
                .and_then(|v| v.to_str())
                .ok_or_else(|| invalid(&format!("{flag} requer um valor")))?;
            Ok(v.to_owned())
        };
        match a {
            "--monitor" => {
                let n = value(a)?
                    .parse::<u32>()
                    .map_err(|_| invalid("--monitor deve ser inteiro >= 0"))?;
                set_once(&mut monitor, n, a)?;
            }
            "--window" => {
                let t = value(a)?;
                if t.trim().is_empty() {
                    return Err(invalid("--window não pode ser vazio").into());
                }
                set_once(&mut window, t, a)?;
            }
            "--zoom" => {
                let z = value(a)?
                    .parse::<f64>()
                    .ok()
                    .filter(|z| z.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(z))
                    .ok_or_else(|| invalid("--zoom deve estar entre 1 e 4"))?;
                set_once(&mut zoom, z, a)?;
            }
            "--mic" => {
                let m = value(a)?;
                if m.trim().is_empty() {
                    return Err(invalid("--mic não pode ser vazio").into());
                }
                set_once(&mut mic, m, a)?;
            }
            _ if a.starts_with('-') && a.parse::<u64>().is_err() && a.len() > 1 => {
                return Err(invalid(&format!("opção desconhecida: {a}")).into());
            }
            _ => {
                // ponytail: one-hour ceiling prevents accidental huge captures; add configurable limits if needed.
                let s = a
                    .parse::<u64>()
                    .ok()
                    .filter(|s| *s <= 3600)
                    .ok_or_else(|| invalid(SECONDS_ERR))?;
                set_once(&mut seconds, s, "seconds")?;
            }
        }
    }
    let target = match (monitor, window) {
        (Some(_), Some(_)) => return Err(invalid("--monitor e --window são exclusivos").into()),
        (Some(n), None) => CaptureTarget::Monitor(n),
        (None, Some(t)) => CaptureTarget::Window(t),
        (None, None) => CaptureTarget::PrimaryMonitor,
    };
    Ok(RecordOpts {
        seconds,
        target,
        zoom,
        mic,
    })
}

/// Microfone só é usado se pedido explicitamente e presente na lista.
fn check_mic(mic: &str, devices: &[String]) -> CliResult<()> {
    if devices.iter().any(|d| d == mic) {
        return Ok(());
    }
    Err(invalid(&format!(
        "microfone '{mic}' não encontrado; use `zoomcast-cli devices`"
    ))
    .into())
}

fn print_devices(devices: &[String], out: &mut dyn io::Write) -> io::Result<()> {
    if devices.is_empty() {
        return writeln!(out, "nenhum microfone encontrado");
    }
    devices.iter().try_for_each(|d| writeln!(out, "{d}"))
}

fn wait_for_stop(seconds: Option<u64>, input: &mut dyn BufRead) -> CliResult<()> {
    if let Some(s) = seconds {
        thread::sleep(Duration::from_secs(s));
        return Ok(());
    }
    eprintln!("gravando; pressione Enter para parar");
    if input.read_line(&mut String::new())? == 0 {
        return Err(invalid("stdin fechado antes de Enter; informe <seconds>").into());
    }
    Ok(())
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

fn export(root: &Path, output: &Path) -> CliResult<()> {
    let project = Project::open(root)?;
    let report = export_project(&project, output, &ExportOptions::default(), &mut |_| {})?;
    println!("{}: {} frames", output.display(), report.frames);
    Ok(())
}

fn record(
    root: &Path,
    opts: &RecordOpts,
    mut backend: Box<dyn CaptureBackend>,
    input: &mut dyn BufRead,
) -> CliResult<()> {
    // Avoid overwriting captured media even if the prior run stopped before writing its manifest.
    if root.join("manifest.json").exists() || root.join("media/screen.mp4").exists() {
        return Err(invalid("projeto já contém captura; escolha outro diretório").into());
    }
    let ffmpeg_bin = ExportOptions::default().ffmpeg_bin;
    backend.start(&CaptureConfig {
        target: opts.target.clone(),
        fps: 30,
        out_dir: root.to_owned(),
        mic: opts.mic.clone(),
        ffmpeg_bin,
    })?;
    let waited = wait_for_stop(opts.seconds, input);
    let captured = backend.stop();
    waited?;
    let captured = captured?;
    let timeline = Timeline {
        zooms: zoomgen::generate(
            &captured.events,
            captured.width,
            captured.height,
            captured.duration_ms,
            ZoomRule {
                scale: opts.zoom.unwrap_or(ZoomRule::default().scale),
                ..ZoomRule::default()
            },
        ),
        ..Timeline::default()
    };
    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        created_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
        media: MediaInfo {
            video_path: captured
                .video_path
                .to_str()
                .ok_or_else(|| invalid("caminho de vídeo inválido"))?
                .replace('\\', "/"),
            width: captured.width,
            height: captured.height,
            fps: captured.fps,
            duration_ms: captured.duration_ms,
            has_audio: captured.has_audio,
        },
        events_path: "events.json".into(),
        timeline,
    };
    let project = Project::create(root, manifest, &captured.events)?;
    println!(
        "{}: {} eventos, {} zooms",
        root.display(),
        captured.events.events.len(),
        project.manifest.timeline.zooms.len()
    );
    Ok(())
}

fn run(args: &[OsString]) -> CliResult<()> {
    let usage = "uso: zoomcast-cli demo <project-dir> [seconds] | record <project-dir> [seconds] [--monitor N | --window TITLE] [--zoom 1-4] [--mic NAME] | export <project-dir> <output.mp4> | devices\n  record sem seconds grava até Enter";
    if let [cmd] = args {
        if cmd == "devices" {
            let devices = list_audio_devices(&ExportOptions::default().ffmpeg_bin)?;
            print_devices(&devices, &mut io::stdout())?;
            return Ok(());
        }
    }
    let [command, root, rest @ ..] = args else {
        return Err(invalid(usage).into());
    };
    let root = PathBuf::from(root);
    match command.to_str() {
        Some("demo")
            if rest.len() <= 1 && rest.iter().all(|a| !a.to_string_lossy().starts_with("--")) =>
        {
            let opts = parse_opts(rest)?;
            let opts = RecordOpts {
                seconds: Some(opts.seconds.unwrap_or(4)),
                ..opts
            };
            record(
                &root,
                &opts,
                Box::new(SyntheticBackend::default()),
                &mut io::empty(),
            )?;
            export(&root, &root.join("render.mp4"))?;
        }
        Some("record") => {
            let opts = parse_opts(rest)?;
            if let Some(m) = &opts.mic {
                check_mic(
                    m,
                    &list_audio_devices(&ExportOptions::default().ffmpeg_bin)?,
                )?;
            }
            record(&root, &opts, default_backend(), &mut io::stdin().lock())?;
        }
        Some("export") if rest.len() == 1 => export(&root, Path::new(&rest[0]))?,
        _ => return Err(invalid(usage).into()),
    }
    Ok(())
}

fn main() {
    if let Err(e) = run(&env::args_os().skip(1).collect::<Vec<_>>()) {
        eprintln!("zoomcast-cli: {e}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use zoomcast_core::events::InputEvent;

    #[test]
    fn demo_persists_and_exports() {
        let root = env::temp_dir().join(format!(
            "zoomcast-cli-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let argv = ["demo".into(), root.clone().into_os_string(), "0".into()];
        run(&argv).unwrap();
        let project = Project::open(&root).unwrap();
        assert!(project.video_file().is_file());
        assert!(root.join("render.mp4").metadata().unwrap().len() > 0);
        assert!(
            project
                .read_events()
                .unwrap()
                .events
                .iter()
                .any(|e| matches!(e, InputEvent::Click { .. }))
        );
        assert_ne!(project.manifest.timeline.zooms, []);
        assert!(run(&argv).is_err()); // Never overwrite captured media.
        let out = root.join("second.mp4");
        run(&[
            "export".into(),
            root.clone().into_os_string(),
            out.clone().into_os_string(),
        ])
        .unwrap();
        assert!(out.metadata().unwrap().len() > 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_args() {
        assert!(run(&["demo".into(), "unused".into(), "3601".into()]).is_err());
        assert!(run(&["record".into(), "unused".into(), "nope".into()]).is_err());
        assert!(run(&["export".into(), "unused".into()]).is_err());
    }

    fn os(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_defaults_and_flags() {
        let o = parse_opts(&os(&[])).unwrap();
        assert_eq!(o.seconds, None);
        assert_eq!(o.target, CaptureTarget::PrimaryMonitor);
        assert!(o.zoom.is_none() && o.mic.is_none());
        let o = parse_opts(&os(&[
            "--zoom",
            "2.5",
            "10",
            "--mic",
            "Mic (USB)",
            "--monitor",
            "1",
        ]))
        .unwrap();
        assert_eq!(o.seconds, Some(10));
        assert_eq!(o.target, CaptureTarget::Monitor(1));
        assert_eq!(o.zoom, Some(2.5));
        assert_eq!(o.mic.as_deref(), Some("Mic (USB)"));
        let o = parse_opts(&os(&["--window", "-x título"])).unwrap();
        assert_eq!(o.target, CaptureTarget::Window("-x título".into()));
    }

    #[test]
    fn rejects_bad_opts() {
        for bad in [
            &["--monitor", "1", "--window", "w"][..],
            &["--window", "w", "--monitor", "1"],
            &["--monitor"],
            &["--monitor", "-1"],
            &["--monitor", "x"],
            &["--monitor", "0", "--monitor", "1"],
            &["--window", "  "],
            &["--mic"],
            &["--mic", " "],
            &["--zoom", "0.5"],
            &["--zoom", "4.5"],
            &["--zoom", "NaN"],
            &["--zoom", "inf"],
            &["--zoom", "abc"],
            &["--zoom", "2", "--zoom", "3"],
            &["--bogus"],
            &["3601"],
            &["-5"],
            &["1", "2"],
        ] {
            assert!(parse_opts(&os(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn mic_must_exist_when_devices_known() {
        let d = vec!["A".to_string(), "B".to_string()];
        assert!(check_mic("A", &d).is_ok());
        assert!(check_mic("C", &d).is_err());
        assert!(check_mic("C", &[]).is_err()); // não fingir áudio se indisponível
    }

    #[test]
    fn devices_output() {
        let mut o = Vec::new();
        print_devices(&["Mic 1".into(), "Mic 2".into()], &mut o).unwrap();
        assert_eq!(String::from_utf8(o).unwrap(), "Mic 1\nMic 2\n");
        let mut o = Vec::new();
        print_devices(&[], &mut o).unwrap();
        assert!(String::from_utf8(o).unwrap().contains("nenhum"));
    }

    #[test]
    fn wait_enter_or_seconds() {
        wait_for_stop(None, &mut io::Cursor::new(b"\n")).unwrap();
        assert!(wait_for_stop(None, &mut io::Cursor::new(b"")).is_err()); // EOF
        wait_for_stop(Some(0), &mut io::Cursor::new(b"")).unwrap();
    }

    struct Spy(
        std::rc::Rc<std::cell::RefCell<Option<CaptureConfig>>>,
        SyntheticBackend,
    );
    impl CaptureBackend for Spy {
        fn start(&mut self, cfg: &CaptureConfig) -> Result<(), zoomcast_capture::CaptureError> {
            *self.0.borrow_mut() = Some(cfg.clone());
            self.1.start(cfg)
        }
        fn stop(
            &mut self,
        ) -> Result<zoomcast_capture::CaptureResult, zoomcast_capture::CaptureError> {
            self.1.stop()
        }
    }

    #[test]
    fn record_forwards_opts() {
        let root = env::temp_dir().join(format!("zoomcast-cli-opts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let seen = std::rc::Rc::default();
        let opts = RecordOpts {
            seconds: None,
            target: CaptureTarget::Monitor(2),
            zoom: Some(3.0),
            mic: Some("Mic".into()),
        };
        record(
            &root,
            &opts,
            Box::new(Spy(std::rc::Rc::clone(&seen), SyntheticBackend::default())),
            &mut io::Cursor::new(b"\n"),
        )
        .unwrap();
        let cfg = seen.borrow().clone().unwrap();
        assert_eq!(cfg.target, CaptureTarget::Monitor(2));
        assert_eq!(cfg.mic.as_deref(), Some("Mic"));
        let p = Project::open(&root).unwrap();
        assert!(
            p.manifest
                .timeline
                .zooms
                .iter()
                .all(|z| (z.scale - 3.0).abs() < 1e-9)
        );
        assert_ne!(p.manifest.timeline.zooms, []);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_rejects_conflicts_before_touching_disk() {
        let root = env::temp_dir().join(format!("zoomcast-cli-conf-{}", std::process::id()));
        let r = root.clone().into_os_string();
        let a = |extra: &[&str]| {
            let mut v: Vec<OsString> = vec!["record".into(), r.clone()];
            v.extend(os(extra));
            v
        };
        assert!(run(&a(&["--monitor", "0", "--window", "x", "1"])).is_err());
        assert!(run(&["demo".into(), r.clone(), "--mic".into(), "m".into()]).is_err());
        assert!(run(&["demo".into(), r.clone(), "--window".into(), "m".into()]).is_err());
        assert!(!root.exists());
        assert!(run(&os(&["devices", "extra"])).is_err());
        run(&os(&["devices"])).unwrap();
    }
}
