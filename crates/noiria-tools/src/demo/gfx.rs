//! デモアセット用の描画基盤（パス塗り・マスク合成）。外部クレート不使用。
//!
//! - `Path`: 直線/ベジェ/Catmull-Rom スプラインのパスを折れ線へ平坦化して保持する。
//! - `Mask`: 符号付き面積累積方式（font-rs 系）で得る AA カバレッジ（0..1）。
//! - `Canvas`: プリマルチプライド f32 RGBA。透過エッジにハローが出ない。
//! - `Ctx`: 設計座標 → デバイス座標のアフィン（拡大・回転・平行移動）を持つ描画コンテキスト。

use image::{Rgba, RgbaImage};

pub type Rgba32 = [f32; 4];

pub fn col(r: u8, g: u8, b: u8) -> Rgba32 {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

pub fn cola(r: u8, g: u8, b: u8, a: f32) -> Rgba32 {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a]
}

pub fn with_alpha(c: Rgba32, a: f32) -> Rgba32 {
    [c[0], c[1], c[2], c[3] * a]
}

pub fn mix(a: Rgba32, b: Rgba32, t: f32) -> Rgba32 {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

pub fn lerpf(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ============================================================
// パス
// ============================================================

#[derive(Clone, Default)]
pub struct Path {
    pub subs: Vec<Vec<(f32, f32)>>,
}

impl Path {
    pub fn new() -> Self {
        Self { subs: Vec::new() }
    }

    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.subs.push(vec![(x, y)]);
        self
    }

    fn cur(&mut self) -> &mut Vec<(f32, f32)> {
        if self.subs.is_empty() {
            self.subs.push(Vec::new());
        }
        self.subs.last_mut().unwrap()
    }

    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.cur().push((x, y));
        self
    }

    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) -> &mut Self {
        let s = *self.cur().last().unwrap_or(&(x, y));
        let n = 16;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let m = 1.0 - t;
            self.cur().push((
                m * m * s.0 + 2.0 * m * t * cx + t * t * x,
                m * m * s.1 + 2.0 * m * t * cy + t * t * y,
            ));
        }
        self
    }

    pub fn cubic_to(&mut self, c1: (f32, f32), c2: (f32, f32), p: (f32, f32)) -> &mut Self {
        let s = *self.cur().last().unwrap_or(&p);
        let n = 24;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let m = 1.0 - t;
            self.cur().push((
                m * m * m * s.0 + 3.0 * m * m * t * c1.0 + 3.0 * m * t * t * c2.0 + t * t * t * p.0,
                m * m * m * s.1 + 3.0 * m * m * t * c1.1 + 3.0 * m * t * t * c2.1 + t * t * t * p.1,
            ));
        }
        self
    }

    /// 現在の部分パスの末尾に、点列を通る Catmull-Rom スプライン（開）を連結する。
    pub fn spline_to(&mut self, pts: &[(f32, f32)]) -> &mut Self {
        let start = *self.cur().last().unwrap_or(&pts[0]);
        let mut all = vec![start];
        all.extend_from_slice(pts);
        let flat = catmull_rom(&all, false);
        self.cur().extend(flat.into_iter().skip(1));
        self
    }

    /// 点列を通る閉じたスプライン（Catmull-Rom）を新しい部分パスとして追加する。
    pub fn closed_spline(pts: &[(f32, f32)]) -> Path {
        Path {
            subs: vec![catmull_rom(pts, true)],
        }
    }

    pub fn open_spline(pts: &[(f32, f32)]) -> Path {
        Path {
            subs: vec![catmull_rom(pts, false)],
        }
    }

    pub fn polygon(pts: &[(f32, f32)]) -> Path {
        Path {
            subs: vec![pts.to_vec()],
        }
    }

    pub fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32, rot: f32) -> Path {
        let n = 64;
        let (s, c) = rot.sin_cos();
        let pts = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let x = a.cos() * rx;
                let y = a.sin() * ry;
                (cx + x * c - y * s, cy + x * s + y * c)
            })
            .collect();
        Path { subs: vec![pts] }
    }

    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Path {
        Path::polygon(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)])
    }

    pub fn append(&mut self, other: &Path) -> &mut Self {
        self.subs.extend(other.subs.iter().cloned());
        self
    }

    pub fn map(&self, f: impl Fn(f32, f32) -> (f32, f32)) -> Path {
        Path {
            subs: self
                .subs
                .iter()
                .map(|s| s.iter().map(|&(x, y)| f(x, y)).collect())
                .collect(),
        }
    }

    pub fn mirrored_x(&self, axis: f32) -> Path {
        self.map(|x, y| (2.0 * axis - x, y))
    }

    /// 変幅ストローク（丸端）。`pts` は (x, y, 太さ)。花弁・睫毛・髪の房の先細りに使う。
    pub fn stroke(pts: &[(f32, f32, f32)]) -> Path {
        // sin(pi*t).powf(..) が終端で -ε → NaN になるため太さを正規化する
        let pts: Vec<(f32, f32, f32)> = pts.iter().map(|&(x, y, w)| (x, y, if w.is_finite() { w.max(0.0) } else { 0.0 })).collect();
        let pts = &pts[..];
        let n = pts.len();
        if n < 2 {
            return Path::new();
        }
        let mut left = Vec::with_capacity(n);
        let mut right = Vec::with_capacity(n);
        for i in 0..n {
            let (x, y, w) = pts[i];
            let a = pts[i.saturating_sub(1)];
            let b = pts[(i + 1).min(n - 1)];
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l = (dx * dx + dy * dy).sqrt().max(1e-6);
            let (nx, ny) = (-dy / l, dx / l);
            left.push((x + nx * w * 0.5, y + ny * w * 0.5));
            right.push((x - nx * w * 0.5, y - ny * w * 0.5));
        }
        let dir_at = |i: usize| {
            let a = pts[i.saturating_sub(1)];
            let b = pts[(i + 1).min(n - 1)];
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l = (dx * dx + dy * dy).sqrt().max(1e-6);
            (dx / l, dy / l)
        };
        let mut poly = left;
        let (d, r) = (dir_at(n - 1), pts[n - 1].2 * 0.5);
        let nn = (-d.1, d.0);
        if r >= 0.3 {
            for k in 1..8 {
                let th = std::f32::consts::PI * k as f32 / 8.0;
                poly.push((
                    pts[n - 1].0 + r * (nn.0 * th.cos() + d.0 * th.sin()),
                    pts[n - 1].1 + r * (nn.1 * th.cos() + d.1 * th.sin()),
                ));
            }
        }
        right.reverse();
        poly.extend(right);
        let (d, r) = (dir_at(0), pts[0].2 * 0.5);
        let nn = (-d.1, d.0);
        if r >= 0.3 {
            for k in 1..8 {
                let th = std::f32::consts::PI * k as f32 / 8.0;
                poly.push((
                    pts[0].0 - r * (nn.0 * th.cos() + d.0 * th.sin()),
                    pts[0].1 - r * (nn.1 * th.cos() + d.1 * th.sin()),
                ));
            }
        }
        Path { subs: vec![poly] }
    }

    /// (x, y, 太さ) の制御点列を通る変幅の管（腕・髪束など）。xy は Catmull-Rom、太さは区間線形。
    pub fn tube(pts: &[(f32, f32, f32)]) -> Path {
        let xy: Vec<(f32, f32)> = pts.iter().map(|p| (p.0, p.1)).collect();
        let flat = catmull_rom(&xy, false);
        let steps = 14usize;
        let sp: Vec<(f32, f32, f32)> = flat
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| {
                let seg = (i / steps).min(pts.len() - 2);
                let t = ((i - seg * steps) as f32 / steps as f32).clamp(0.0, 1.0);
                (x, y, lerpf(pts[seg].2, pts[seg + 1].2, t))
            })
            .collect();
        Path::stroke(&sp)
    }

    /// 点列を通るスプライン上に、均一太さ→先細りの太さ関数で変幅ストロークを作る。
    pub fn stroke_spline(pts: &[(f32, f32)], width: impl Fn(f32) -> f32) -> Path {
        let flat = catmull_rom(pts, false);
        let n = flat.len();
        let sp: Vec<(f32, f32, f32)> = flat
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| (x, y, width(i as f32 / (n - 1).max(1) as f32)))
            .collect();
        Path::stroke(&sp)
    }
}

pub fn catmull_rom(pts: &[(f32, f32)], closed: bool) -> Vec<(f32, f32)> {
    let n = pts.len();
    if n < 3 {
        return pts.to_vec();
    }
    let get = |i: i32| -> (f32, f32) {
        if closed {
            pts[((i % n as i32) + n as i32) as usize % n]
        } else {
            pts[i.clamp(0, n as i32 - 1) as usize]
        }
    };
    let segs = if closed { n } else { n - 1 };
    let mut out = Vec::new();
    for i in 0..segs as i32 {
        let (p0, p1, p2, p3) = (get(i - 1), get(i), get(i + 1), get(i + 2));
        let steps = 14;
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            let t2 = t * t;
            let t3 = t2 * t;
            let f = |a: f32, b: f32, c: f32, d: f32| {
                0.5 * (2.0 * b + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3)
            };
            out.push((f(p0.0, p1.0, p2.0, p3.0), f(p0.1, p1.1, p2.1, p3.1)));
        }
    }
    if !closed {
        out.push(pts[n - 1]);
    }
    out
}

// ============================================================
// マスク（AA カバレッジ）
// ============================================================

#[derive(Clone)]
pub struct Mask {
    pub x0: i32,
    pub y0: i32,
    pub w: usize,
    pub h: usize,
    pub a: Vec<f32>,
}

impl Mask {
    pub fn empty() -> Mask {
        Mask {
            x0: 0,
            y0: 0,
            w: 0,
            h: 0,
            a: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> f32 {
        let lx = x - self.x0;
        let ly = y - self.y0;
        if lx < 0 || ly < 0 || lx >= self.w as i32 || ly >= self.h as i32 {
            0.0
        } else {
            self.a[ly as usize * self.w + lx as usize]
        }
    }

    /// デバイス座標のパスをラスタライズ。`bounds` = (w, h) でキャンバス範囲に切る。
    pub fn from_path(path: &Path, bounds: (usize, usize)) -> Mask {
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for s in &path.subs {
            for &(x, y) in s {
                if !x.is_finite() || !y.is_finite() {
                    continue;
                }
                minx = minx.min(x);
                miny = miny.min(y);
                maxx = maxx.max(x);
                maxy = maxy.max(y);
            }
        }
        if minx > maxx {
            return Mask::empty();
        }
        let x0 = (minx.floor() as i32 - 1).max(0);
        let y0 = (miny.floor() as i32 - 1).max(0);
        let x1 = (maxx.ceil() as i32 + 1).min(bounds.0 as i32);
        let y1 = (maxy.ceil() as i32 + 1).min(bounds.1 as i32);
        if x1 <= x0 || y1 <= y0 {
            return Mask::empty();
        }
        let w = (x1 - x0) as usize;
        let h = (y1 - y0) as usize;
        let mut acc = vec![0.0f32; (w + 3) * h];
        let stride = w + 3;
        let ox = x0 as f32;
        let oy = y0 as f32;
        for s in &path.subs {
            let n = s.len();
            if n < 2 {
                continue;
            }
            for i in 0..n {
                let a = s[i];
                let b = s[(i + 1) % n];
                draw_line(&mut acc, stride, w, h, (a.0 - ox, a.1 - oy), (b.0 - ox, b.1 - oy));
            }
        }
        let mut out = vec![0.0f32; w * h];
        for y in 0..h {
            let mut sum = 0.0f32;
            for x in 0..w {
                sum += acc[y * stride + x];
                out[y * w + x] = sum.abs().min(1.0);
            }
        }
        Mask { x0, y0, w, h, a: out }
    }

    pub fn full(w: usize, h: usize) -> Mask {
        Mask {
            x0: 0,
            y0: 0,
            w,
            h,
            a: vec![1.0; w * h],
        }
    }

    /// 積（クリップ）。結果の範囲は両者の交差。
    pub fn mul(&self, o: &Mask) -> Mask {
        if self.is_empty() || o.is_empty() {
            return Mask::empty();
        }
        let x0 = self.x0.max(o.x0);
        let y0 = self.y0.max(o.y0);
        let x1 = (self.x0 + self.w as i32).min(o.x0 + o.w as i32);
        let y1 = (self.y0 + self.h as i32).min(o.y0 + o.h as i32);
        if x1 <= x0 || y1 <= y0 {
            return Mask::empty();
        }
        let w = (x1 - x0) as usize;
        let h = (y1 - y0) as usize;
        let mut a = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x0 + x as i32, y0 + y as i32);
                a[y * w + x] = self.get(px, py) * o.get(px, py);
            }
        }
        Mask { x0, y0, w, h, a }
    }

    /// self から o を引く（self * (1 - o)）。
    pub fn sub(&self, o: &Mask) -> Mask {
        let mut m = self.clone();
        for y in 0..m.h {
            for x in 0..m.w {
                let v = o.get(m.x0 + x as i32, m.y0 + y as i32);
                m.a[y * m.w + x] *= 1.0 - v;
            }
        }
        m
    }

    /// 和（max 合成）。範囲は和集合。
    pub fn union(&self, o: &Mask) -> Mask {
        if self.is_empty() {
            return o.clone();
        }
        if o.is_empty() {
            return self.clone();
        }
        let x0 = self.x0.min(o.x0);
        let y0 = self.y0.min(o.y0);
        let x1 = (self.x0 + self.w as i32).max(o.x0 + o.w as i32);
        let y1 = (self.y0 + self.h as i32).max(o.y0 + o.h as i32);
        let w = (x1 - x0) as usize;
        let h = (y1 - y0) as usize;
        let mut a = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x0 + x as i32, y0 + y as i32);
                let (u, v) = (self.get(px, py), o.get(px, py));
                a[y * w + x] = u + v - u * v;
            }
        }
        Mask { x0, y0, w, h, a }
    }

    pub fn scaled(&self, k: f32) -> Mask {
        let mut m = self.clone();
        for v in m.a.iter_mut() {
            *v *= k;
        }
        m
    }

    pub fn shifted(&self, dx: i32, dy: i32) -> Mask {
        let mut m = self.clone();
        m.x0 += dx;
        m.y0 += dy;
        m
    }

    /// 半径 r（px）のボックスブラー 3 回（ほぼガウシアン）。範囲は r*3 だけ拡張。
    pub fn blur(&self, r: f32) -> Mask {
        if self.is_empty() || r < 0.4 {
            return self.clone();
        }
        let pad = (r * 2.6).ceil() as i32 + 1;
        let w = self.w + 2 * pad as usize;
        let h = self.h + 2 * pad as usize;
        let mut buf = vec![0.0f32; w * h];
        for y in 0..self.h {
            for x in 0..self.w {
                buf[(y + pad as usize) * w + x + pad as usize] = self.a[y * self.w + x];
            }
        }
        let k = (r * 0.8).max(1.0) as usize;
        for _ in 0..3 {
            box_blur_h(&mut buf, w, h, k);
            box_blur_v(&mut buf, w, h, k);
        }
        Mask {
            x0: self.x0 - pad,
            y0: self.y0 - pad,
            w,
            h,
            a: buf,
        }
    }

    /// 円形カーネルの最大値フィルタで膨張（輪郭線用）。r は小さい値（<=6）を想定。
    pub fn dilate(&self, r: f32) -> Mask {
        if self.is_empty() {
            return self.clone();
        }
        let pad = r.ceil() as i32 + 1;
        let w = self.w + 2 * pad as usize;
        let h = self.h + 2 * pad as usize;
        let mut out = vec![0.0f32; w * h];
        let ri = r.ceil() as i32;
        let mut offs = Vec::new();
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                if d <= r + 0.5 {
                    let wgt = (r + 0.5 - d).clamp(0.0, 1.0);
                    offs.push((dx, dy, wgt));
                }
            }
        }
        let x0 = self.x0 - pad;
        let y0 = self.y0 - pad;
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x0 + x as i32, y0 + y as i32);
                let mut best = 0.0f32;
                for &(dx, dy, wgt) in &offs {
                    let v = self.get(px + dx, py + dy) * wgt;
                    if v > best {
                        best = v;
                    }
                }
                out[y * w + x] = best;
            }
        }
        Mask {
            x0,
            y0,
            w,
            h,
            a: out,
        }
    }
}

fn box_blur_h(buf: &mut [f32], w: usize, h: usize, k: usize) {
    let mut tmp = vec![0.0f32; w];
    let win = (2 * k + 1) as f32;
    for y in 0..h {
        let row = &mut buf[y * w..(y + 1) * w];
        let mut sum = 0.0f32;
        for (i, t) in tmp.iter_mut().enumerate() {
            let lo = i as i32 - k as i32;
            let hi = i + k;
            if i == 0 {
                sum = (0..=k.min(w - 1)).map(|j| row[j]).sum();
            } else {
                if hi < w {
                    sum += row[hi];
                }
                if lo > 0 {
                    sum -= row[(lo - 1) as usize];
                }
            }
            *t = sum / win;
        }
        row.copy_from_slice(&tmp);
    }
}

fn box_blur_v(buf: &mut [f32], w: usize, h: usize, k: usize) {
    let mut tmp = vec![0.0f32; h];
    let win = (2 * k + 1) as f32;
    for x in 0..w {
        let mut sum = 0.0f32;
        for (i, t) in tmp.iter_mut().enumerate() {
            let lo = i as i32 - k as i32;
            let hi = i + k;
            if i == 0 {
                sum = (0..=k.min(h - 1)).map(|j| buf[j * w + x]).sum();
            } else {
                if hi < h {
                    sum += buf[hi * w + x];
                }
                if lo > 0 {
                    sum -= buf[(lo - 1) as usize * w + x];
                }
            }
            *t = sum / win;
        }
        for (i, t) in tmp.iter().enumerate() {
            buf[i * w + x] = *t;
        }
    }
}

/// font-rs 方式の符号付き面積累積。x は [0, w] へクリップ（範囲外は縁に張り付けた垂直線として加算）。
fn draw_line(acc: &mut [f32], stride: usize, w: usize, h: usize, p0: (f32, f32), p1: (f32, f32)) {
    if (p0.1 - p1.1).abs() < 1e-9 {
        return;
    }
    let (dir, a, b) = if p0.1 < p1.1 { (1.0f32, p0, p1) } else { (-1.0f32, p1, p0) };
    let dxdy = (b.0 - a.0) / (b.1 - a.1);
    let mut x = a.0;
    let ystart = a.1.max(0.0);
    if a.1 < 0.0 {
        x -= a.1 * dxdy;
    }
    let yend = b.1.min(h as f32);
    if ystart >= yend {
        return;
    }
    let wf = w as f32;
    let y_first = ystart.floor() as usize;
    let y_last = (yend.ceil() as usize).min(h);
    for y in y_first..y_last {
        let dy = ((y + 1) as f32).min(yend) - (y as f32).max(ystart);
        if dy <= 0.0 {
            continue;
        }
        let xnext = x + dxdy * dy;
        let d = dy * dir;
        // クリップ
        let (xa, xb) = (x.clamp(0.0, wf), xnext.clamp(0.0, wf));
        let base = y * stride;
        let (x0, x1) = if xa < xb { (xa, xb) } else { (xb, xa) };
        let x0floor = x0.floor();
        let x0i = x0floor as usize;
        let x1ceil = x1.ceil();
        let x1i = x1ceil as usize;
        if x1i <= x0i + 1 {
            let xmf = 0.5 * (xa + xb) - x0floor;
            acc[base + x0i] += d - d * xmf;
            acc[base + x0i + 1] += d * xmf;
        } else {
            let s = 1.0 / (x1 - x0);
            let x0f = x0 - x0floor;
            let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
            let x1f = x1 - x1ceil + 1.0;
            let am = 0.5 * s * x1f * x1f;
            acc[base + x0i] += d * a0;
            if x1i == x0i + 2 {
                acc[base + x0i + 1] += d * (1.0 - a0 - am);
            } else {
                let a1 = s * (1.5 - x0f);
                acc[base + x0i + 1] += d * (a1 - a0);
                for xi in x0i + 2..x1i - 1 {
                    acc[base + xi] += d * s;
                }
                let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                acc[base + x1i - 1] += d * (1.0 - a2 - am);
            }
            acc[base + x1i] += d * am;
        }
        x = xnext;
    }
}

// ============================================================
// キャンバス
// ============================================================

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    /// プリマルチプライド RGBA
    pub px: Vec<[f32; 4]>,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![[0.0; 4]; w * h],
        }
    }

    pub fn clone_canvas(&self) -> Canvas {
        Canvas {
            w: self.w,
            h: self.h,
            px: self.px.clone(),
        }
    }

    pub fn fill_all(&mut self, f: impl Fn(f32, f32) -> Rgba32) {
        for y in 0..self.h {
            for x in 0..self.w {
                let c = f(x as f32 + 0.5, y as f32 + 0.5);
                self.px[y * self.w + x] = [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
            }
        }
    }

    #[inline]
    pub fn over(&mut self, x: usize, y: usize, c: Rgba32, cov: f32) {
        let a = c[3] * cov;
        if a <= 0.0 {
            return;
        }
        let p = &mut self.px[y * self.w + x];
        let k = 1.0 - a;
        p[0] = c[0] * a + p[0] * k;
        p[1] = c[1] * a + p[1] * k;
        p[2] = c[2] * a + p[2] * k;
        p[3] = a + p[3] * k;
    }

    /// 加算合成（光の表現）。アルファは増やさない。
    #[inline]
    pub fn add(&mut self, x: usize, y: usize, c: [f32; 3], amt: f32) {
        let p = &mut self.px[y * self.w + x];
        p[0] += c[0] * amt;
        p[1] += c[1] * amt;
        p[2] += c[2] * amt;
    }

    pub fn fill_mask(&mut self, m: &Mask, f: &dyn Fn(f32, f32) -> Rgba32) {
        for y in 0..m.h {
            let py = m.y0 + y as i32;
            if py < 0 || py >= self.h as i32 {
                continue;
            }
            for x in 0..m.w {
                let cov = m.a[y * m.w + x];
                if cov <= 0.001 {
                    continue;
                }
                let px = m.x0 + x as i32;
                if px < 0 || px >= self.w as i32 {
                    continue;
                }
                let c = f(px as f32 + 0.5, py as f32 + 0.5);
                self.over(px as usize, py as usize, c, cov);
            }
        }
    }

    pub fn fill_solid(&mut self, m: &Mask, c: Rgba32) {
        self.fill_mask(m, &|_, _| c);
    }

    pub fn glow(&mut self, cx: f32, cy: f32, r: f32, c: [f32; 3], amt: f32, pow: f32) {
        let x0 = ((cx - r).floor().max(0.0)) as usize;
        let x1 = ((cx + r).ceil().min(self.w as f32)) as usize;
        let y0 = ((cy - r).floor().max(0.0)) as usize;
        let y1 = ((cy + r).ceil().min(self.h as f32)) as usize;
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt() / r;
                if d < 1.0 {
                    let t = (1.0 - d).powf(pow);
                    self.add(x, y, c, t * amt);
                }
            }
        }
    }

    /// 8bit RGBA へ。不透明キャンバスなら `dither=true` で ±0.5LSB のディザをかける。
    pub fn to_image(&self, dither: bool) -> RgbaImage {
        let mut img = RgbaImage::new(self.w as u32, self.h as u32);
        for y in 0..self.h {
            for x in 0..self.w {
                let p = self.px[y * self.w + x];
                let a = p[3].clamp(0.0, 1.0);
                if a <= 0.0 {
                    img.put_pixel(x as u32, y as u32, Rgba([0, 0, 0, 0]));
                    continue;
                }
                let n = if dither {
                    crate::paint::dither_noise(x as u32, y as u32) * 0.5
                } else {
                    0.0
                };
                let f = |v: f32| ((v / a).clamp(0.0, 1.0) * 255.0 + n).round().clamp(0.0, 255.0) as u8;
                if (a * 255.0).round() < 1.0 {
                    img.put_pixel(x as u32, y as u32, Rgba([0, 0, 0, 0]));
                    continue;
                }
                img.put_pixel(
                    x as u32,
                    y as u32,
                    Rgba([f(p[0]), f(p[1]), f(p[2]), (a * 255.0).round() as u8]),
                );
            }
        }
        img
    }
}

// ============================================================
// 描画コンテキスト（設計座標 → デバイス座標）
// ============================================================

#[derive(Clone, Copy)]
pub struct Xf {
    pub s: f32,
    pub rot: f32,
    pub tx: f32,
    pub ty: f32,
    /// 回転・拡大の中心（設計座標）
    pub cx: f32,
    pub cy: f32,
}

impl Xf {
    /// 設計座標をそのまま 1:1 で使う。
    pub fn identity() -> Xf {
        Xf {
            s: 1.0,
            rot: 0.0,
            tx: 0.0,
            ty: 0.0,
            cx: 0.0,
            cy: 0.0,
        }
    }

    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let (sn, cs) = self.rot.sin_cos();
        let (dx, dy) = (x - self.cx, y - self.cy);
        (
            self.cx + self.tx + (dx * cs - dy * sn) * self.s,
            self.cy + self.ty + (dx * sn + dy * cs) * self.s,
        )
    }

    pub fn inverse(&self, x: f32, y: f32) -> (f32, f32) {
        let (sn, cs) = self.rot.sin_cos();
        let (dx, dy) = ((x - self.cx - self.tx) / self.s, (y - self.cy - self.ty) / self.s);
        (self.cx + dx * cs + dy * sn, self.cy - dx * sn + dy * cs)
    }
}

pub struct Ctx<'a> {
    pub cv: &'a mut Canvas,
    pub xf: Xf,
}

impl<'a> Ctx<'a> {
    pub fn new(cv: &'a mut Canvas, xf: Xf) -> Self {
        Self { cv, xf }
    }

    pub fn mask(&self, p: &Path) -> Mask {
        let xf = self.xf;
        Mask::from_path(&p.map(|x, y| xf.apply(x, y)), (self.cv.w, self.cv.h))
    }

    /// デバイス px = 設計 px × scale のぼかし半径換算。
    pub fn blur(&self, m: &Mask, r: f32) -> Mask {
        m.blur(r * self.xf.s)
    }

    pub fn dilate(&self, m: &Mask, r: f32) -> Mask {
        m.dilate(r * self.xf.s)
    }

    pub fn shift(&self, m: &Mask, dx: f32, dy: f32) -> Mask {
        let s = self.xf.s;
        m.shifted((dx * s).round() as i32, (dy * s).round() as i32)
    }

    pub fn fill(&mut self, m: &Mask, c: Rgba32) {
        self.cv.fill_solid(m, c);
    }

    /// 設計座標のクロージャで塗る（グラデーション等）。
    pub fn fill_fn(&mut self, m: &Mask, f: &dyn Fn(f32, f32) -> Rgba32) {
        let xf = self.xf;
        self.cv.fill_mask(m, &|x, y| {
            let (dx, dy) = xf.inverse(x, y);
            f(dx, dy)
        });
    }

    pub fn fill_path(&mut self, p: &Path, c: Rgba32) -> Mask {
        let m = self.mask(p);
        self.fill(&m, c);
        m
    }

    /// 輪郭線（外側へ `w` 設計 px）を先に敷いてから本体を塗る。
    pub fn fill_outlined(&mut self, m: &Mask, c: &dyn Fn(f32, f32) -> Rgba32, outline: Rgba32, w: f32) {
        if w > 0.0 {
            let d = self.dilate(m, w);
            let ring = d.sub(&m.scaled(1.0));
            self.fill(&ring, outline);
        }
        self.fill_fn(m, c);
    }

    /// `m` の内側にだけ塗る（クリップ付き塗り）。
    pub fn fill_clipped(&mut self, shape: &Mask, clip: &Mask, c: Rgba32) {
        let m = shape.mul(clip);
        self.fill(&m, c);
    }
}

pub fn rgb_hex(h: u32) -> Rgba32 {
    col((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

/// 折れ線を法線方向へ `d` だけ平行移動した折れ線（襟の縁取り・裾線用）。
pub fn offset_pts(flat: &[(f32, f32)], d: f32) -> Vec<(f32, f32)> {
    let n = flat.len();
    (0..n)
        .map(|i| {
            let a = flat[i.saturating_sub(1)];
            let b = flat[(i + 1).min(n - 1)];
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l = (dx * dx + dy * dy).sqrt().max(1e-6);
            (flat[i].0 - dy / l * d, flat[i].1 + dx / l * d)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tapered_stroke_stays_thin_and_closed() {
        // sin(pi*t).powf(..) の終端 NaN で太さが矩形に膨らむ不具合の回帰テスト
        let p = Path::stroke_spline(&[(150.0, 150.0), (146.0, 210.0), (152.0, 270.0), (166.0, 312.0)], |t| {
            5.5 * (std::f32::consts::PI * t).sin().powf(0.8) + 0.4
        });
        let m = Mask::from_path(&p, (500, 900));
        let row = (200 - m.y0) as usize;
        let covered = (0..m.w).filter(|&x| m.a[row * m.w + x] > 0.5).count();
        assert!((1..=8).contains(&covered), "stroke width at y=200 is {covered}px");
    }
}
