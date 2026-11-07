use crate::{Error, Result, TimeMs};
use serde::{Deserialize, Serialize};

pub const MIN_SCALE: f64 = 1.0;
pub const MAX_SCALE: f64 = 4.0;
pub const DEFAULT_RAMP_MS: TimeMs = 400;

/// Zoom em `[start_ms, end_ms)`. `center` normalizado (0..1) no quadro. As rampas de
/// entrada/saída ocorrem **dentro** do intervalo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoomSegment {
    pub id: u32,
    pub start_ms: TimeMs,
    pub end_ms: TimeMs,
    pub scale: f64,
    pub center_x: f64,
    pub center_y: f64,
    #[serde(default = "default_ramp")]
    pub ramp_ms: TimeMs,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn default_ramp() -> TimeMs {
    DEFAULT_RAMP_MS
}
fn yes() -> bool {
    true
}

impl ZoomSegment {
    /// # Errors
    /// `InvalidSegment` se intervalo vazio, escala fora de [1,4], centro fora de [0,1] ou não finito.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: &str| Err(Error::InvalidSegment(format!("id {}: {m}", self.id)));
        if self.end_ms <= self.start_ms {
            return bad("end_ms <= start_ms");
        }
        if !(self.scale.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(&self.scale)) {
            return bad("scale fora de [1,4]");
        }
        for c in [self.center_x, self.center_y] {
            if !(c.is_finite() && (0.0..=1.0).contains(&c)) {
                return bad("centro fora de [0,1]");
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.start_ms < other.end_ms && self.end_ms > other.start_ms
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    pub trim_start_ms: TimeMs,
    /// `None` = até o fim da mídia.
    pub trim_end_ms: Option<TimeMs>,
    pub zooms: Vec<ZoomSegment>,
}

impl Timeline {
    /// Adiciona segmento mantendo ordem por início. Regra: sem sobreposição entre segmentos
    /// (inclusive desabilitados, para o usuário reativá-los sem conflito).
    ///
    /// # Errors
    /// `InvalidSegment` ou `Overlap`.
    pub fn add_zoom(&mut self, seg: ZoomSegment) -> Result<()> {
        seg.validate()?;
        if let Some(o) = self
            .zooms
            .iter()
            .find(|z| z.id == seg.id || z.overlaps(&seg))
        {
            return Err(Error::Overlap(o.id));
        }
        self.zooms.push(seg);
        self.zooms.sort_by_key(|z| z.start_ms);
        Ok(())
    }

    pub fn remove_zoom(&mut self, id: u32) -> bool {
        let n = self.zooms.len();
        self.zooms.retain(|z| z.id != id);
        n != self.zooms.len()
    }

    /// Janela efetiva de saída `(início, fim)` dado `duration_ms` da mídia.
    ///
    /// # Errors
    /// `InvalidTrim` se a janela for vazia.
    pub fn effective_trim(&self, duration_ms: TimeMs) -> Result<(TimeMs, TimeMs)> {
        let s = self.trim_start_ms.min(duration_ms);
        let e = self.trim_end_ms.map_or(duration_ms, |e| e.min(duration_ms));
        if e <= s {
            return Err(Error::InvalidTrim(format!("janela vazia [{s},{e}]")));
        }
        Ok((s, e))
    }

    #[must_use]
    pub fn next_id(&self) -> u32 {
        self.zooms.iter().map(|z| z.id).max().map_or(1, |m| m + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn seg(id: u32, s: TimeMs, e: TimeMs) -> ZoomSegment {
        ZoomSegment {
            id,
            start_ms: s,
            end_ms: e,
            scale: 2.0,
            center_x: 0.5,
            center_y: 0.5,
            ramp_ms: 400,
            enabled: true,
        }
    }

    #[test]
    fn rejects_overlap_and_duplicate_id() {
        let mut t = Timeline::default();
        t.add_zoom(seg(1, 1000, 3000)).unwrap();
        assert!(matches!(
            t.add_zoom(seg(2, 2999, 4000)),
            Err(Error::Overlap(1))
        ));
        assert!(matches!(
            t.add_zoom(seg(1, 5000, 6000)),
            Err(Error::Overlap(1))
        ));
        t.add_zoom(seg(2, 3000, 4000)).unwrap(); // adjacência permitida
        assert_eq!(t.zooms.iter().map(|z| z.id).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn validates_bounds() {
        let mut s = seg(1, 0, 10);
        s.scale = 5.0;
        assert!(s.validate().is_err());
        s.scale = f64::NAN;
        assert!(s.validate().is_err());
        assert!(seg(1, 10, 10).validate().is_err());
    }

    #[test]
    fn trim_rules() {
        let t = Timeline {
            trim_start_ms: 500,
            trim_end_ms: Some(99_999),
            ..Default::default()
        };
        assert_eq!(t.effective_trim(10_000).unwrap(), (500, 10_000));
        let t = Timeline {
            trim_start_ms: 20_000,
            ..Default::default()
        };
        assert_eq!(
            t.effective_trim(10_000).unwrap_err().to_string(),
            "trim inválido: janela vazia [10000,10000]"
        );
    }
}
