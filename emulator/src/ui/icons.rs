//! Vector icons rasterised on the CPU at their exact on-screen size from
//! signed distance functions, which gives clean anti-aliasing at any size.

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Cross,
    CircleBtn,
    Square,
    Triangle,
    Play,
    DPad,
    Stick,
    CursorFill,
    CursorOutline,
    Sliders,
    Refresh,
    Exit,
    Image,
    Keyboard,
    Check,
    ChevronLeft,
    ChevronRight,
    Spinner,
    Search,
    Download,
    Clock,
    Warning,
    Globe,
}

type P = (f32, f32);

fn len(p: P) -> f32 {
    (p.0 * p.0 + p.1 * p.1).sqrt()
}

/// Distance from `p` to segment `a`-`b`.
fn seg(p: P, a: P, b: P) -> f32 {
    let pa = (p.0 - a.0, p.1 - a.1);
    let ba = (b.0 - a.0, b.1 - a.1);
    let h = ((pa.0 * ba.0 + pa.1 * ba.1) / (ba.0 * ba.0 + ba.1 * ba.1)).clamp(0.0, 1.0);
    len((pa.0 - ba.0 * h, pa.1 - ba.1 * h))
}

/// Signed distance to a closed polygon (negative inside).
fn polygon(p: P, pts: &[P]) -> f32 {
    let mut d = f32::MAX;
    let mut inside = false;
    let n = pts.len();
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        d = d.min(seg(p, a, b));
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    if inside { -d } else { d }
}

/// Signed distance to an axis-aligned box centred at `c` with half extents `h`,
/// with corner radius `r`.
fn rbox(p: P, c: P, h: P, r: f32) -> f32 {
    let q = ((p.0 - c.0).abs() - h.0 + r, (p.1 - c.1).abs() - h.1 + r);
    len((q.0.max(0.0), q.1.max(0.0))) + q.0.max(q.1).min(0.0) - r
}

fn circle(p: P, c: P, r: f32) -> f32 {
    len((p.0 - c.0, p.1 - c.1)) - r
}

fn polyline(p: P, pts: &[P]) -> f32 {
    pts.windows(2).map(|w| seg(p, w[0], w[1])).fold(f32::MAX, f32::min)
}

/// Signed distance in unit space (icon box is 0..1) for each icon.
fn sdf(icon: Icon, p: P, px: f32) -> f32 {
    // Stroke half-width in unit space; at least ~0.75px on screen.
    let stroke = (0.055f32).max(0.75 / px);
    match icon {
        Icon::Cross => {
            let d = seg(p, (0.24, 0.24), (0.76, 0.76)).min(seg(p, (0.76, 0.24), (0.24, 0.76)));
            d - stroke
        }
        Icon::CircleBtn => (circle(p, (0.5, 0.5), 0.29)).abs() - stroke,
        Icon::Square => rbox(p, (0.5, 0.5), (0.27, 0.27), 0.02).abs() - stroke,
        Icon::Triangle => {
            let t = [(0.5, 0.2), (0.82, 0.74), (0.18, 0.74)];
            polygon(p, &t).abs() - stroke
        }
        Icon::Play => polygon(p, &[(0.3, 0.18), (0.84, 0.5), (0.3, 0.82)]) - 0.02,
        Icon::DPad => {
            let v = rbox(p, (0.5, 0.5), (0.14, 0.4), 0.06);
            let h = rbox(p, (0.5, 0.5), (0.4, 0.14), 0.06);
            v.min(h)
        }
        Icon::Stick => {
            let ring = circle(p, (0.5, 0.5), 0.36).abs() - stroke * 0.9;
            ring.min(circle(p, (0.5, 0.5), 0.17))
        }
        Icon::CursorFill | Icon::CursorOutline => {
            let arrow = [
                (0.2, 0.08),
                (0.2, 0.84),
                (0.39, 0.66),
                (0.52, 0.94),
                (0.64, 0.88),
                (0.51, 0.61),
                (0.76, 0.61),
            ];
            let d = polygon(p, &arrow);
            if icon == Icon::CursorFill { d } else { d - 0.07 }
        }
        Icon::Sliders => {
            let mut d = f32::MAX;
            for (y, knob) in [(0.26, 0.68), (0.5, 0.34), (0.74, 0.58)] {
                d = d.min(seg(p, (0.14, y), (0.86, y)) - stroke * 0.8);
                d = d.min(circle(p, (knob, y), 0.09));
            }
            d
        }
        Icon::Refresh => {
            let c = (0.5, 0.52);
            let r = 0.3;
            let (dx, dy) = (p.0 - c.0, p.1 - c.1);
            let ang = dy.atan2(dx);
            // Arc from ~-60deg sweeping clockwise to ~250deg, with a gap top-right.
            let in_gap = ang > -1.35 && ang < -0.35;
            let arc = if in_gap {
                let a0 = (c.0 + r * (-1.35f32).cos(), c.1 + r * (-1.35f32).sin());
                let a1 = (c.0 + r * (-0.35f32).cos(), c.1 + r * (-0.35f32).sin());
                len((p.0 - a0.0, p.1 - a0.1)).min(len((p.0 - a1.0, p.1 - a1.1)))
            } else {
                (len((dx, dy)) - r).abs()
            } - stroke;
            let tip = (c.0 + r * (-0.35f32).cos(), c.1 + r * (-0.35f32).sin());
            let head = polygon(p, &[(tip.0 + 0.13, tip.1 - 0.02), (tip.0 - 0.05, tip.1 - 0.16), (tip.0 - 0.04, tip.1 + 0.08)]);
            arc.min(head)
        }
        Icon::Exit => {
            let frame = polyline(p, &[(0.5, 0.2), (0.2, 0.2), (0.2, 0.8), (0.5, 0.8)]) - stroke;
            let shaft = seg(p, (0.38, 0.5), (0.8, 0.5)) - stroke;
            let head = polyline(p, &[(0.66, 0.35), (0.82, 0.5), (0.66, 0.65)]) - stroke;
            frame.min(shaft).min(head)
        }
        Icon::Image => {
            let frame = rbox(p, (0.5, 0.5), (0.36, 0.3), 0.06).abs() - stroke;
            let sun = circle(p, (0.36, 0.38), 0.07);
            let hills = polygon(p, &[(0.2, 0.74), (0.42, 0.5), (0.56, 0.62), (0.66, 0.54), (0.8, 0.74)]);
            frame.min(sun).min(hills.max(rbox(p, (0.5, 0.5), (0.34, 0.28), 0.04)))
        }
        Icon::Keyboard => {
            let frame = rbox(p, (0.5, 0.5), (0.4, 0.26), 0.07).abs() - stroke * 0.8;
            let mut keys = f32::MAX;
            for row in 0..2 {
                for col in 0..4 {
                    let c = (0.26 + col as f32 * 0.16, 0.4 + row as f32 * 0.13);
                    keys = keys.min(rbox(p, c, (0.035, 0.03), 0.01));
                }
            }
            let space = rbox(p, (0.5, 0.66), (0.2, 0.03), 0.01);
            frame.min(keys).min(space)
        }
        Icon::Check => polyline(p, &[(0.22, 0.52), (0.42, 0.72), (0.8, 0.3)]) - stroke * 1.2,
        Icon::ChevronLeft => polyline(p, &[(0.62, 0.22), (0.36, 0.5), (0.62, 0.78)]) - stroke * 1.1,
        Icon::ChevronRight => polyline(p, &[(0.38, 0.22), (0.64, 0.5), (0.38, 0.78)]) - stroke * 1.1,
        Icon::Search => {
            let lens = circle(p, (0.43, 0.43), 0.22).abs() - stroke * 1.1;
            lens.min(seg(p, (0.6, 0.6), (0.8, 0.8)) - stroke * 1.5)
        }
        Icon::Download => {
            let shaft = seg(p, (0.5, 0.16), (0.5, 0.58)) - stroke * 1.1;
            let head = polyline(p, &[(0.32, 0.42), (0.5, 0.6), (0.68, 0.42)]) - stroke * 1.1;
            let tray = polyline(p, &[(0.18, 0.62), (0.18, 0.82), (0.82, 0.82), (0.82, 0.62)]) - stroke;
            shaft.min(head).min(tray)
        }
        Icon::Clock => {
            let face = circle(p, (0.5, 0.5), 0.34).abs() - stroke;
            face.min(polyline(p, &[(0.5, 0.3), (0.5, 0.52), (0.64, 0.6)]) - stroke)
        }
        Icon::Warning => {
            let t = polygon(p, &[(0.5, 0.16), (0.86, 0.8), (0.14, 0.8)]).abs() - stroke;
            let bar = seg(p, (0.5, 0.4), (0.5, 0.58)) - stroke * 1.1;
            t.min(bar).min(circle(p, (0.5, 0.69), stroke * 1.4))
        }
        Icon::Globe => {
            let face = circle(p, (0.5, 0.5), 0.34);
            // Meridian ellipse and the equator, clipped to the face.
            let (ex, ey) = ((p.0 - 0.5) / 0.15, (p.1 - 0.5) / 0.34);
            let meridian = ((ex * ex + ey * ey).sqrt() - 1.0).abs() * 0.15 - stroke * 0.8;
            let equator = seg(p, (0.16, 0.5), (0.84, 0.5)) - stroke * 0.8;
            (face.abs() - stroke).min(meridian.min(equator).max(face))
        }
        Icon::Spinner => {
            // A 270 degree arc; rotation is done by drawing different frames.
            let (dx, dy) = (p.0 - 0.5, p.1 - 0.5);
            let ang = dy.atan2(dx);
            if ang > 0.0 && ang < 1.5708 {
                let e0 = (0.5 + 0.34, 0.5);
                let e1 = (0.5, 0.5 + 0.34);
                len((p.0 - e0.0, p.1 - e0.1)).min(len((p.0 - e1.0, p.1 - e1.1))) - stroke * 1.2
            } else {
                (len((dx, dy)) - 0.34).abs() - stroke * 1.2
            }
        }
    }
}

/// Coverage mask of `icon` at `n`x`n` pixels.
pub fn rasterize(icon: Icon, n: usize) -> Vec<u8> {
    let px = n as f32;
    let mut out = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let p = ((x as f32 + 0.5) / px, (y as f32 + 0.5) / px);
            // Convert the unit-space distance to pixels for a 1px AA ramp.
            let d = sdf(icon, p, px) * px;
            let cov = (0.5 - d).clamp(0.0, 1.0);
            out[y * n + x] = (cov * 255.0 + 0.5) as u8;
        }
    }
    out
}
