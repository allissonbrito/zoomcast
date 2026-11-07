//! Recorte + reamostragem bilinear de um frame RGB24. Puro, sem I/O.
use zoomcast_core::camera::Camera;

/// Janela de recorte em pixels (coordenadas contínuas) dentro do quadro de origem.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Janela `size/scale` centrada em `center` (normalizado), limitada para caber no quadro.
#[must_use]
pub fn crop_rect(cam: &Camera, width: u32, height: u32) -> Rect {
    let scale = if cam.scale.is_finite() {
        cam.scale.max(1.0)
    } else {
        1.0
    };
    let (fw, fh) = (f64::from(width), f64::from(height));
    let (w, h) = (fw / scale, fh / scale);
    let place = |c: f64, full: f64, win: f64| {
        let c = if c.is_finite() { c } else { 0.5 };
        (c * full - win / 2.0).clamp(0.0, full - win)
    };
    Rect {
        x: place(cam.center_x, fw, w),
        y: place(cam.center_y, fh, h),
        w,
        h,
    }
}

/// Escala `rect` de `src` (`width`x`height` RGB24) para `dst` (mesmas dimensões), bilinear com
/// convenção de centro de pixel. `rect` cheio = cópia exata.
///
/// # Panics
/// Se `src`/`dst` não tiverem `width*height*3` bytes.
pub fn render_frame(src: &[u8], width: u32, height: u32, rect: &Rect, dst: &mut [u8]) {
    let (w, h) = (width as usize, height as usize);
    assert_eq!(src.len(), w * h * 3, "src com tamanho errado");
    assert_eq!(dst.len(), w * h * 3, "dst com tamanho errado");
    if w == 0 || h == 0 {
        return;
    }
    if rect.x == 0.0 && rect.y == 0.0 && rect.w >= f64::from(width) && rect.h >= f64::from(height) {
        dst.copy_from_slice(src);
        return;
    }
    // Tabela por coluna: (índice*3 esquerdo, índice*3 direito, peso do direito).
    let taps = |n: usize, start: f64, len: f64| -> Vec<(usize, usize, f32)> {
        (0..n)
            .map(|o| {
                let s =
                    (start + (o as f64 + 0.5) * len / n as f64 - 0.5).clamp(0.0, (n - 1) as f64);
                let i0 = s.floor() as usize;
                (i0, (i0 + 1).min(n - 1), (s - i0 as f64) as f32)
            })
            .collect()
    };
    let xs = taps(w, rect.x, rect.w);
    let ys = taps(h, rect.y, rect.h);
    for (oy, &(y0, y1, fy)) in ys.iter().enumerate() {
        let (r0, r1) = (
            &src[y0 * w * 3..(y0 + 1) * w * 3],
            &src[y1 * w * 3..(y1 + 1) * w * 3],
        );
        let out = &mut dst[oy * w * 3..(oy + 1) * w * 3];
        for (px, &(x0, x1, fx)) in out.chunks_exact_mut(3).zip(&xs) {
            for (c, o) in px.iter_mut().enumerate() {
                let top = lerp(r0[x0 * 3 + c], r0[x1 * 3 + c], fx);
                let bot = lerp(r1[x0 * 3 + c], r1[x1 * 3 + c], fx);
                *o = (top + (bot - top) * fy + 0.5) as u8;
            }
        }
    }
}

#[inline]
fn lerp(a: u8, b: u8, t: f32) -> f32 {
    let a = f32::from(a);
    a + (f32::from(b) - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(scale: f64, cx: f64, cy: f64) -> Camera {
        Camera {
            scale,
            center_x: cx,
            center_y: cy,
        }
    }

    #[test]
    fn full_scale_is_exact_identity() {
        let (w, h) = (8u32, 6u32);
        let src: Vec<u8> = (0..w * h * 3).map(|i| (i * 7 % 251) as u8).collect();
        let r = crop_rect(&Camera::FULL, w, h);
        assert_eq!(
            r,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 8.0,
                h: 6.0
            }
        );
        let mut dst = vec![0; src.len()];
        render_frame(&src, w, h, &r, &mut dst);
        assert_eq!(src, dst);
        // Caminho bilinear geral também é identidade (retângulo cheio, sem atalho).
        let r2 = Rect {
            w: 8.0 - 1e-12,
            ..r
        };
        render_frame(&src, w, h, &r2, &mut dst);
        assert_eq!(src, dst);
    }

    #[test]
    fn center_is_clamped() {
        let r = crop_rect(&cam(2.0, 1.0, 0.0), 100, 80);
        assert_eq!((r.x, r.y, r.w, r.h), (50.0, 0.0, 50.0, 40.0));
        let r = crop_rect(&cam(2.0, -3.0, 9.0), 100, 80);
        assert_eq!((r.x, r.y), (0.0, 40.0));
        let r = crop_rect(&cam(4.0, 0.5, 0.5), 100, 80);
        assert_eq!((r.x, r.y, r.w, r.h), (37.5, 30.0, 25.0, 20.0));
    }

    #[test]
    fn zoom_on_linear_ramp_is_exact() {
        // Rampa horizontal linear: bilinear reproduz a função afim longe das bordas.
        let (w, h) = (64u32, 4u32);
        let mut src = vec![0u8; (w * h * 3) as usize];
        for px in src.chunks_exact_mut(3).enumerate() {
            let x = (px.0 as u32 % w) as u8;
            px.1.fill(x * 4);
        }
        let r = crop_rect(&cam(2.0, 0.5, 0.5), w, h);
        let mut dst = vec![0; src.len()];
        render_frame(&src, w, h, &r, &mut dst);
        for ox in 8..56usize {
            let expect = (r.x + (ox as f64 + 0.5) * r.w / 64.0 - 0.5) * 4.0;
            let got = f64::from(dst[ox * 3]);
            assert!(
                (got - expect).abs() <= 0.6,
                "ox={ox} got={got} expect={expect}"
            );
        }
    }
}
