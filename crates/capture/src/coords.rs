//! Geometria e tempo, funções puras (testáveis em Linux).
//! Coordenadas de tela = pixels físicos do desktop virtual (processo DPI-aware).
//! Coordenadas de vídeo = pixels do MP4, origem topo-esquerda da região capturada.

use zoomcast_core::TimeMs;
use zoomcast_core::events::{InputEvent, MouseButton};

/// Retângulo em pixels de tela. `x`/`y` podem ser negativos (monitor à esquerda/acima do primário).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Region {
    fn right(self) -> i64 {
        i64::from(self.x) + i64::from(self.w)
    }
    fn bottom(self) -> i64 {
        i64::from(self.y) + i64::from(self.h)
    }

    /// Interseção; `None` se vazia.
    #[must_use]
    pub fn clip(self, bounds: Region) -> Option<Region> {
        let l = i64::from(self.x).max(i64::from(bounds.x));
        let t = i64::from(self.y).max(i64::from(bounds.y));
        let r = self.right().min(bounds.right());
        let b = self.bottom().min(bounds.bottom());
        if r <= l || b <= t {
            return None;
        }
        Some(Region {
            x: i32::try_from(l).ok()?,
            y: i32::try_from(t).ok()?,
            w: u32::try_from(r - l).ok()?,
            h: u32::try_from(b - t).ok()?,
        })
    }

    /// Recorta em `bounds` e arredonda w/h para par (yuv420p/libx264 exigem). `None` se < 2x2.
    #[must_use]
    pub fn fit(self, bounds: Region) -> Option<Region> {
        let c = self.clip(bounds)?;
        let r = Region {
            w: c.w & !1,
            h: c.h & !1,
            ..c
        };
        (r.w >= 2 && r.h >= 2).then_some(r)
    }

    /// Ponto de tela → pixel de vídeo; `None` se fora da região.
    #[must_use]
    pub fn to_video(self, sx: i32, sy: i32) -> Option<(f64, f64)> {
        let (x, y) = (i64::from(sx), i64::from(sy));
        let inside = x >= i64::from(self.x)
            && x < self.right()
            && y >= i64::from(self.y)
            && y < self.bottom();
        inside.then(|| {
            (
                f64::from(sx) - f64::from(self.x),
                f64::from(sy) - f64::from(self.y),
            )
        })
    }

    /// Como `to_video`, mas prende às bordas (cursor fora da região aparece na borda).
    #[must_use]
    pub fn clamp_to_video(self, sx: i32, sy: i32) -> (f64, f64) {
        let mx = f64::from(self.w.saturating_sub(1));
        let my = f64::from(self.h.saturating_sub(1));
        (
            (f64::from(sx) - f64::from(self.x)).clamp(0.0, mx),
            (f64::from(sy) - f64::from(self.y)).clamp(0.0, my),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub region: Region,
    pub primary: bool,
}

/// `None` = primário. `Some(n)` = n-ésimo (0-based) ordenado esquerda→direita, depois topo→base
/// (ordem estável; a do `EnumDisplayMonitors` não é garantida).
#[must_use]
pub fn select_monitor(monitors: &[MonitorRect], index: Option<u32>) -> Option<Region> {
    match index {
        None => monitors.iter().find(|m| m.primary).map(|m| m.region),
        Some(n) => {
            let mut v: Vec<Region> = monitors.iter().map(|m| m.region).collect();
            v.sort_by_key(|r| (r.x, r.y));
            v.get(usize::try_from(n).ok()?).copied()
        }
    }
}

fn with_t(e: InputEvent, t_ms: TimeMs) -> InputEvent {
    match e {
        InputEvent::Move { x, y, .. } => InputEvent::Move { t_ms, x, y },
        InputEvent::Click { x, y, button, .. } => InputEvent::Click { t_ms, x, y, button },
    }
}

/// Converte do relógio dos eventos (t0 = spawn do ffmpeg) para o do vídeo (t0 = 1º frame):
/// `t_video = t_evento - offset_ms`. Eventos < 0: cliques descartados; o último `Move`
/// vira `t=0` (posição inicial do cursor). Eventos > `max_t_ms` descartados.
#[must_use]
pub fn apply_offset(events: &[InputEvent], offset_ms: i64, max_t_ms: TimeMs) -> Vec<InputEvent> {
    let mut pre: Option<InputEvent> = None;
    let mut out = Vec::with_capacity(events.len());
    for &e in events {
        let t = i64::try_from(e.t_ms())
            .unwrap_or(i64::MAX)
            .saturating_sub(offset_ms);
        match u64::try_from(t) {
            Ok(t) if t <= max_t_ms => out.push(with_t(e, t)),
            Ok(_) => {}
            Err(_) => {
                if matches!(e, InputEvent::Move { .. }) {
                    pre = Some(with_t(e, 0));
                }
            }
        }
    }
    if let Some(p) = pre {
        out.insert(0, p);
    }
    out
}

/// Amostrador de mouse: recebe (tempo, posição de tela, botões) e emite eventos em coords de vídeo.
/// Botões na ordem `[Left, Right, Middle]` (já lógicos, após troca de botões do SO).
#[derive(Debug)]
pub struct Sampler {
    region: Region,
    pos: Option<(i32, i32)>,
    down: [bool; 3],
}

impl Sampler {
    #[must_use]
    pub fn new(region: Region) -> Self {
        Self {
            region,
            pos: None,
            down: [false; 3],
        }
    }

    /// `Move` quando a posição muda (1ª amostra sempre emite); `Click` na borda de descida,
    /// só se dentro da região.
    pub fn step(&mut self, t_ms: TimeMs, sx: i32, sy: i32, down: [bool; 3]) -> Vec<InputEvent> {
        let mut out = Vec::new();
        if self.pos != Some((sx, sy)) {
            self.pos = Some((sx, sy));
            let (x, y) = self.region.clamp_to_video(sx, sy);
            out.push(InputEvent::Move { t_ms, x, y });
        }
        for (i, button) in [MouseButton::Left, MouseButton::Right, MouseButton::Middle]
            .into_iter()
            .enumerate()
        {
            if down[i] && !self.down[i] {
                if let Some((x, y)) = self.region.to_video(sx, sy) {
                    out.push(InputEvent::Click { t_ms, x, y, button });
                }
            }
        }
        self.down = down;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Region = Region {
        x: -1920,
        y: 0,
        w: 1920,
        h: 1080,
    };

    #[test]
    fn clip_and_even() {
        let b = Region {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        assert_eq!(
            Region {
                x: -10,
                y: -10,
                w: 101,
                h: 101
            }
            .clip(b),
            Some(Region {
                x: 0,
                y: 0,
                w: 91,
                h: 91
            })
        );
        assert_eq!(
            Region {
                x: 0,
                y: 0,
                w: 101,
                h: 51
            }
            .fit(b),
            Some(Region {
                x: 0,
                y: 0,
                w: 100,
                h: 50
            })
        );
        assert_eq!(
            Region {
                x: 2000,
                y: 0,
                w: 10,
                h: 10
            }
            .clip(b),
            None
        );
        assert_eq!(
            Region {
                x: 0,
                y: 0,
                w: 1,
                h: 10
            }
            .fit(b),
            None
        );
    }

    #[test]
    fn to_video_negative_origin() {
        assert_eq!(R.to_video(-1920, 0), Some((0.0, 0.0)));
        assert_eq!(R.to_video(-1, 1079), Some((1919.0, 1079.0)));
        assert_eq!(R.to_video(0, 0), None);
        assert_eq!(R.clamp_to_video(500, -5), (1919.0, 0.0));
    }

    #[test]
    fn monitors() {
        let m = |x, primary| MonitorRect {
            region: Region {
                x,
                y: 0,
                w: 100,
                h: 100,
            },
            primary,
        };
        let ms = [m(0, true), m(-100, false), m(100, false)];
        assert_eq!(select_monitor(&ms, None).map(|r| r.x), Some(0));
        assert_eq!(select_monitor(&ms, Some(0)).map(|r| r.x), Some(-100));
        assert_eq!(select_monitor(&ms, Some(2)).map(|r| r.x), Some(100));
        assert_eq!(select_monitor(&ms, Some(3)), None);
        assert_eq!(select_monitor(&[], None), None);
    }

    #[test]
    fn offset() {
        let mv = |t_ms, x| InputEvent::Move { t_ms, x, y: 0.0 };
        let ck = |t_ms| InputEvent::Click {
            t_ms,
            x: 1.0,
            y: 1.0,
            button: MouseButton::Left,
        };
        let ev = [mv(0, 1.0), ck(10), mv(20, 2.0), ck(150), mv(500, 3.0)];
        let o = apply_offset(&ev, 100, 300);
        assert_eq!(o, vec![mv(0, 2.0), ck(50)]);
        // offset negativo atrasa eventos
        assert_eq!(apply_offset(&[mv(5, 1.0)], -10, 100), vec![mv(15, 1.0)]);
    }

    #[test]
    fn sampler() {
        let r = Region {
            x: 100,
            y: 100,
            w: 200,
            h: 200,
        };
        let mut s = Sampler::new(r);
        assert_eq!(
            s.step(0, 150, 150, [false; 3]),
            vec![InputEvent::Move {
                t_ms: 0,
                x: 50.0,
                y: 50.0
            }]
        );
        assert_eq!(s.step(8, 150, 150, [false; 3]).len(), 0);
        // clique: 1 evento na descida, nenhum enquanto segura
        let e = s.step(16, 150, 150, [true, false, false]);
        assert_eq!(
            e,
            vec![InputEvent::Click {
                t_ms: 16,
                x: 50.0,
                y: 50.0,
                button: MouseButton::Left
            }]
        );
        assert_eq!(s.step(24, 150, 150, [true, false, false]).len(), 0);
        // fora da região: move preso à borda, clique descartado
        let e = s.step(32, 0, 0, [true, true, false]);
        assert_eq!(
            e,
            vec![InputEvent::Move {
                t_ms: 32,
                x: 0.0,
                y: 0.0
            }]
        );
    }
}
