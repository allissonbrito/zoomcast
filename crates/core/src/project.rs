//! Pacote de projeto: diretório `<nome>.zoomcast/` com `manifest.json`, `events.json` e mídia original
//! preservada em `media/`. Gravação atômica (tmp + rename) para salvamento recuperável.
use crate::{Error, Result, TimeMs, events::EventLog, timeline::Timeline};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub const FORMAT_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    /// Relativo à raiz do pacote, sem `..` nem caminho absoluto.
    pub video_path: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_ms: TimeMs,
    pub has_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    /// Instante de criação, ms desde Unix epoch.
    pub created_unix_ms: u64,
    pub media: MediaInfo,
    pub events_path: String,
    pub timeline: Timeline,
}

#[derive(Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
}

fn safe_rel(p: &str) -> Result<PathBuf> {
    let pb = PathBuf::from(p);
    let ok = !p.is_empty() && pb.components().all(|c| matches!(c, Component::Normal(_)));
    if ok {
        Ok(pb)
    } else {
        Err(Error::UnsafePath(p.to_owned()))
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

impl Project {
    #[must_use]
    pub fn video_file(&self) -> PathBuf {
        self.root.join(&self.manifest.media.video_path)
    }

    /// Cria o diretório do pacote e grava manifesto + eventos.
    ///
    /// # Errors
    /// Caminhos inseguros, e/s ou json.
    pub fn create(root: &Path, manifest: Manifest, events: &EventLog) -> Result<Self> {
        safe_rel(&manifest.media.video_path)?;
        safe_rel(&manifest.events_path)?;
        fs::create_dir_all(root.join("media"))?;
        let p = Self {
            root: root.to_owned(),
            manifest,
        };
        p.write_events(events)?;
        p.save()?;
        Ok(p)
    }

    /// # Errors
    /// e/s ou json.
    pub fn save(&self) -> Result<()> {
        write_atomic(
            &self.root.join(MANIFEST_FILE),
            &serde_json::to_vec_pretty(&self.manifest)?,
        )
    }

    /// # Errors
    /// e/s ou json.
    pub fn write_events(&self, events: &EventLog) -> Result<()> {
        let mut ev = events.clone();
        ev.sort();
        write_atomic(
            &self.root.join(safe_rel(&self.manifest.events_path)?),
            &serde_json::to_vec(&ev)?,
        )
    }

    /// # Errors
    /// Versão desconhecida (rejeitada, sem tentativa de migração), caminho inseguro, e/s ou json.
    pub fn open(root: &Path) -> Result<Self> {
        let manifest: Manifest = serde_json::from_slice(&fs::read(root.join(MANIFEST_FILE))?)?;
        if manifest.format_version != FORMAT_VERSION {
            return Err(Error::UnsupportedVersion {
                found: manifest.format_version,
                supported: FORMAT_VERSION,
            });
        }
        safe_rel(&manifest.media.video_path)?;
        safe_rel(&manifest.events_path)?;
        for z in &manifest.timeline.zooms {
            z.validate()?;
        }
        Ok(Self {
            root: root.to_owned(),
            manifest,
        })
    }

    /// # Errors
    /// e/s ou json.
    pub fn read_events(&self) -> Result<EventLog> {
        Ok(serde_json::from_slice(&fs::read(
            self.root.join(safe_rel(&self.manifest.events_path)?),
        )?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{InputEvent, MouseButton};

    fn manifest() -> Manifest {
        Manifest {
            format_version: FORMAT_VERSION,
            created_unix_ms: 1,
            media: MediaInfo {
                video_path: "media/screen.mp4".into(),
                width: 1280,
                height: 720,
                fps: 30.0,
                duration_ms: 5000,
                has_audio: false,
            },
            events_path: "events.json".into(),
            timeline: Timeline::default(),
        }
    }

    #[test]
    fn roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("a.zoomcast");
        let ev = EventLog {
            events: vec![InputEvent::Click {
                t_ms: 10,
                x: 1.0,
                y: 2.0,
                button: MouseButton::Left,
            }],
        };
        let p = Project::create(&root, manifest(), &ev).unwrap();
        let q = Project::open(&root).unwrap();
        assert_eq!(p.manifest, q.manifest);
        assert_eq!(q.read_events().unwrap(), ev);
        assert!(!root.join("manifest.tmp").exists());
    }

    #[test]
    fn rejects_future_version_and_path_traversal() {
        let d = tempfile::tempdir().unwrap();
        let mut m = manifest();
        m.format_version = 99;
        fs::write(
            d.path().join(MANIFEST_FILE),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            Project::open(d.path()),
            Err(Error::UnsupportedVersion { found: 99, .. })
        ));
        let mut m = manifest();
        m.media.video_path = "../../etc/passwd".into();
        assert!(matches!(
            Project::create(&d.path().join("x"), m, &EventLog::default()),
            Err(Error::UnsafePath(_))
        ));
    }
}
