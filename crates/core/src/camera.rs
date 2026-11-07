//! Função de câmera: fonte da verdade do enquadramento em cada instante.
//! O exportador deve reproduzir exatamente esta curva (ver testes de conformidade em `export`).
use crate::{TimeMs, timeline::Timeline, timeline::ZoomSegment};

/// Enquadramento: `scale` >= 1 e centro normalizado já **limitado** para a janela não sair do quadro.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub scale: f64,
    pub center_x: f64,
    pub center_y: f64,
}

impl Camera {
    pub const FULL: Self = Self {
        scale: 1.0,
        center_x: 0.5,
        center_y: 0.5,
    };
}

#[must_use]
pub fn smoothstep(p: f64) -> f64 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

/// Envelope 0..1 do segmento em `t_ms` (rampa de entrada e saída suaves dentro do intervalo).
#[must_use]
pub fn envelope(seg: &ZoomSegment, t_ms: f64) -> f64 {
    let (s, e) = (seg.start_ms as f64, seg.end_ms as f64);
    if t_ms < s || t_ms >= e {
        return 0.0;
    }
    let ramp = (seg.ramp_ms as f64).min((e - s) / 2.0).max(1.0);
    smoothstep((t_ms - s) / ramp).min(smoothstep((e - t_ms) / ramp))
}

/// Limita o centro para que a janela `1/scale` caiba no quadro.
#[must_use]
pub fn clamp_center(c: f64, scale: f64) -> f64 {
    let half = 0.5 / scale;
    c.clamp(half, 1.0 - half)
}

#[must_use]
pub fn camera_at(tl: &Timeline, t_ms: TimeMs) -> Camera {
    camera_at_f(tl, t_ms as f64)
}

#[must_use]
pub fn camera_at_f(tl: &Timeline, t_ms: f64) -> Camera {
    // Sem sobreposição => no máximo um segmento ativo.
    let Some((seg, env)) = tl
        .zooms
        .iter()
        .filter(|z| z.enabled)
        .map(|z| (z, envelope(z, t_ms)))
        .find(|(_, e)| *e > 0.0)
    else {
        return Camera::FULL;
    };
    let scale = 1.0 + (seg.scale - 1.0) * env;
    let cx = 0.5 + (seg.center_x - 0.5) * env;
    let cy = 0.5 + (seg.center_y - 0.5) * env;
    Camera {
        scale,
        center_x: clamp_center(cx, scale),
        center_y: clamp_center(cy, scale),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::ZoomSegment;

    fn tl() -> Timeline {
        Timeline {
            zooms: vec![ZoomSegment {
                id: 1,
                start_ms: 1000,
                end_ms: 4000,
                scale: 2.0,
                center_x: 0.9,
                center_y: 0.1,
                ramp_ms: 400,
                enabled: true,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn full_outside_and_at_edges() {
        let t = tl();
        assert_eq!(camera_at(&t, 0), Camera::FULL);
        assert_eq!(camera_at(&t, 1000), Camera::FULL);
        assert_eq!(camera_at(&t, 4000), Camera::FULL);
    }

    #[test]
    fn holds_target_and_clamps_center() {
        let c = camera_at(&tl(), 2500);
        assert!((c.scale - 2.0).abs() < 1e-9);
        assert!((c.center_x - 0.75).abs() < 1e-9); // 0.9 limitado a 1-0.25
        assert!((c.center_y - 0.25).abs() < 1e-9);
    }

    #[test]
    fn ramp_is_monotonic() {
        let t = tl();
        let mut prev = 1.0;
        for ms in (1000..=1400).step_by(20) {
            let s = camera_at(&t, ms).scale;
            assert!(s >= prev - 1e-12);
            prev = s;
        }
        assert!((prev - 2.0).abs() < 1e-9);
    }

    #[test]
    fn disabled_is_ignored() {
        let mut t = tl();
        t.zooms[0].enabled = false;
        assert_eq!(camera_at(&t, 2500), Camera::FULL);
    }
}
