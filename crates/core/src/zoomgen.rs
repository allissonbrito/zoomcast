//! Geração automática de zooms a partir de cliques (regra inicial, ajustável pelo usuário).
use crate::{
    TimeMs,
    events::{EventLog, InputEvent},
    timeline::{DEFAULT_RAMP_MS, ZoomSegment},
};

#[derive(Debug, Clone, Copy)]
pub struct ZoomRule {
    pub scale: f64,
    /// Antecedência do início do zoom em relação ao clique.
    pub lead_ms: TimeMs,
    /// Permanência após o clique.
    pub hold_ms: TimeMs,
    pub ramp_ms: TimeMs,
}

impl Default for ZoomRule {
    fn default() -> Self {
        Self {
            scale: 2.0,
            lead_ms: 600,
            hold_ms: 1600,
            ramp_ms: DEFAULT_RAMP_MS,
        }
    }
}

/// Um segmento por clique, centrado no clique. Cliques cujo intervalo colide com o segmento
/// anterior **truncam** o anterior (sem sobreposição).
/// ponytail: sem fusão/pan entre cliques próximos; upgrade = centro animado dentro do segmento.
#[must_use]
pub fn generate(
    log: &EventLog,
    video_w: u32,
    video_h: u32,
    duration_ms: TimeMs,
    rule: ZoomRule,
) -> Vec<ZoomSegment> {
    if video_w == 0 || video_h == 0 {
        return vec![];
    }
    let mut clicks: Vec<(TimeMs, f64, f64)> = log
        .events
        .iter()
        .filter_map(|e| match *e {
            InputEvent::Click { t_ms, x, y, .. } if x.is_finite() && y.is_finite() => {
                Some((t_ms, x, y))
            }
            _ => None,
        })
        .collect();
    clicks.sort_by_key(|c| c.0);

    let mut out: Vec<ZoomSegment> = Vec::new();
    for (t, x, y) in clicks {
        let start = t.saturating_sub(rule.lead_ms);
        let end = (t + rule.hold_ms).min(duration_ms);
        if end <= start {
            continue;
        }
        if let Some(prev) = out.last_mut() {
            if start < prev.end_ms {
                prev.end_ms = start;
            }
        }
        out.retain(|s| s.end_ms > s.start_ms);
        out.push(ZoomSegment {
            id: 0,
            start_ms: start,
            end_ms: end,
            scale: rule.scale,
            center_x: (x / f64::from(video_w)).clamp(0.0, 1.0),
            center_y: (y / f64::from(video_h)).clamp(0.0, 1.0),
            ramp_ms: rule.ramp_ms,
            enabled: true,
        });
    }
    for (i, s) in out.iter_mut().enumerate() {
        s.id = i as u32 + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{events::MouseButton, timeline::Timeline};

    fn click(t: TimeMs, x: f64, y: f64) -> InputEvent {
        InputEvent::Click {
            t_ms: t,
            x,
            y,
            button: MouseButton::Left,
        }
    }

    #[test]
    fn centers_and_windows() {
        let log = EventLog {
            events: vec![click(5000, 960.0, 270.0)],
        };
        let z = generate(&log, 1920, 1080, 20_000, ZoomRule::default());
        assert_eq!(z.len(), 1);
        assert_eq!((z[0].start_ms, z[0].end_ms), (4400, 6600));
        assert!((z[0].center_x - 0.5).abs() < 1e-9 && (z[0].center_y - 0.25).abs() < 1e-9);
    }

    #[test]
    fn close_clicks_never_overlap_and_all_valid() {
        let log = EventLog {
            events: vec![
                click(3000, 100.0, 100.0),
                click(3500, 1800.0, 900.0),
                click(900, 5.0, 5.0),
            ],
        };
        let z = generate(&log, 1920, 1080, 10_000, ZoomRule::default());
        let mut tl = Timeline::default();
        for s in z {
            tl.add_zoom(s)
                .expect("gerado deve ser válido e sem sobreposição");
        }
        assert!(tl.zooms.len() >= 2);
    }

    #[test]
    fn clamps_to_duration_and_ignores_zero_size() {
        let log = EventLog {
            events: vec![click(9900, 1.0, 1.0)],
        };
        assert_eq!(
            generate(&log, 1920, 1080, 10_000, ZoomRule::default())[0].end_ms,
            10_000
        );
        assert_eq!(generate(&log, 0, 0, 10_000, ZoomRule::default()).len(), 0);
    }
}
