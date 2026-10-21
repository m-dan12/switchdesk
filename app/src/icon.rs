//! Иконка приложения: два «экрана»; закрашенный показывает, где сейчас ваш ввод.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconState {
    /// Ввод на этом компьютере.
    Local,
    /// Ввод ушёл на другой компьютер.
    Remote,
    /// Этим компьютером управляет другой.
    Hosting,
}

fn bg(state: IconState) -> [f32; 3] {
    match state {
        IconState::Local => [0.30, 0.55, 1.00],
        IconState::Remote => [0.21, 0.77, 0.49],
        IconState::Hosting => [0.95, 0.66, 0.23],
    }
}

/// Расстояние со знаком до скруглённого прямоугольника (центр, полуразмеры, радиус).
fn sdf(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// RGBA `size`×`size` с мягкими краями.
pub fn render(state: IconState, size: u32) -> Vec<u8> {
    let s = size as f32;
    let u = s / 64.0;
    let (left_solid, right_solid) = match state {
        IconState::Local => (true, false),
        IconState::Remote => (false, true),
        IconState::Hosting => (true, true),
    };
    let ss = 3; // сэмплов на сторону пикселя
    let bgc = bg(state);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0, 0.0, 0.0);
            for sy in 0..ss {
                for sx in 0..ss {
                    let px = x as f32 + (sx as f32 + 0.5) / ss as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / ss as f32;
                    // фон
                    let d_bg = sdf(px, py, 32.0 * u, 32.0 * u, 32.0 * u, 32.0 * u, 14.0 * u);
                    if d_bg > 0.0 {
                        continue;
                    }
                    let (mut cr, mut cg, mut cb) = (bgc[0], bgc[1], bgc[2]);
                    for (cx, cy, solid) in [(19.0, 29.0, left_solid), (45.0, 35.0, right_solid)] {
                        let d = sdf(px, py, cx * u, cy * u, 11.0 * u, 9.0 * u, 3.0 * u);
                        let on = if solid { d <= 0.0 } else { d <= 0.0 && d >= -2.2 * u };
                        if on {
                            (cr, cg, cb) = (1.0, 1.0, 1.0);
                        }
                    }
                    r += cr;
                    g += cg;
                    b += cb;
                    a += 1.0;
                }
            }
            let n = (ss * ss) as f32;
            let cov = a / n;
            if a > 0.0 {
                out.extend_from_slice(&[
                    (r / a * 255.0) as u8,
                    (g / a * 255.0) as u8,
                    (b / a * 255.0) as u8,
                    (cov * 255.0) as u8,
                ]);
            } else {
                out.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_expected_size_and_transparent_corners() {
        let px = render(IconState::Local, 32);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(px[3], 0, "угол прозрачен");
        let center = (16 * 32 + 16) * 4;
        assert_eq!(px[center + 3], 255, "центр непрозрачен");
    }

    #[test]
    fn states_differ() {
        assert_ne!(render(IconState::Local, 32), render(IconState::Remote, 32));
        assert_ne!(render(IconState::Local, 32), render(IconState::Hosting, 32));
    }
}
