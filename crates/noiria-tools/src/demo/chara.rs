//! 立ち絵（朝比奈ひなた / 月城しおり）の手続き描画。フラット塗りの素朴な図形キャラ。
//!
//! 設計座標は 500x900（頭頂 y≈60、下端は膝上）。`Ctx` のアフィンで CG（縮小）にも流用する。
//! 4 表情で輪郭を一致させるため、体・髪（前髪含む）は `draw_base` で 1 回だけ描き、
//! 表情パーツは「顔 − 前髪」マスクでクリップして上から重ねる（alpha は不変）。
//!
//! スタイル規約（回帰テストで色数を縛る）: グラデ・複数ハイライト・まつ毛・髪の筋・顔の陰影は描かない。
//! 影は 1 段のベタ影まで。頭と体は別の座標系（`Lay`）に載せ、頭は傾け、体は個別のポーズで左右非対称にする。

use super::gfx::*;

type P = (f32, f32);
/// 前髪の房（各房は (x, y, 太さ) の折れ線）。
type Locks = Vec<Vec<(f32, f32, f32)>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Who {
    Hinata,
    Shiori,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expr {
    Normal,
    Smile,
    Blush,
    Sad,
}

/// 画風。A = フラット・ミニマル（既定）、B = 絵本・切り紙風、C = A の 5 頭身版。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Style {
    #[default]
    A,
    B,
    C,
}

impl Style {
    pub fn parse(s: &str) -> Option<Style> {
        match s.to_ascii_lowercase().as_str() {
            "a" => Some(Style::A),
            "b" => Some(Style::B),
            "c" => Some(Style::C),
            _ => None,
        }
    }
}

/// CG 用のポーズ指定。立ち絵は `Opts::default()`。
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Pose {
    #[default]
    Stand,
    /// ひなたの上げた腕を空へ伸ばす（指差し）。
    Sky,
}

#[derive(Clone, Copy, Default)]
pub struct Opts {
    pub pose: Pose,
    /// 頭の傾き（rad）の上書き。
    pub head_ang: Option<f32>,
}

struct Pal {
    skin: Rgba32,
    skin_shade: Rgba32,
    cheek: Rgba32,
    blush: Rgba32,
    hair: Rgba32,
    hair_back: Rgba32,
    brow: Rgba32,
    eye: Rgba32,
    mouth: Rgba32,
    mouth_in: Rgba32,
    tongue: Rgba32,
    navy: Rgba32,
    navy_shade: Rgba32,
    white: Rgba32,
    white_shade: Rgba32,
    ribbon: Rgba32,
    ribbon_dark: Rgba32,
    card: Rgba32,
    card_shade: Rgba32,
    shoe: Rgba32,
    clip: Rgba32,
    paper_shadow: Rgba32,
}

fn pal(who: Who, style: Style) -> Pal {
    let mut p = Pal {
        skin: col(255, 228, 208),
        skin_shade: col(240, 198, 178),
        cheek: col(253, 190, 180),
        blush: col(247, 152, 152),
        hair: col(0, 0, 0),
        hair_back: col(0, 0, 0),
        brow: col(0, 0, 0),
        eye: col(0, 0, 0),
        mouth: col(176, 78, 80),
        mouth_in: col(150, 52, 60),
        tongue: col(236, 120, 128),
        navy: col(42, 54, 110),
        navy_shade: col(32, 40, 88),
        white: col(250, 250, 253),
        white_shade: col(214, 220, 238),
        ribbon: col(0, 0, 0),
        ribbon_dark: col(0, 0, 0),
        card: col(228, 202, 158),
        card_shade: col(204, 174, 130),
        shoe: col(92, 64, 60),
        clip: col(246, 202, 74),
        paper_shadow: cola(70, 48, 56, 0.30),
    };
    match who {
        Who::Hinata => {
            p.hair = col(186, 114, 66);
            p.hair_back = col(152, 88, 52);
            p.brow = col(124, 72, 46);
            p.eye = col(78, 46, 36);
            p.ribbon = col(224, 62, 72);
            p.ribbon_dark = col(174, 42, 58);
        }
        Who::Shiori => {
            p.hair = col(48, 52, 88);
            p.hair_back = col(34, 38, 68);
            p.brow = col(56, 58, 96);
            p.eye = col(76, 72, 142);
            p.ribbon = col(70, 112, 216);
            p.ribbon_dark = col(48, 80, 170);
        }
    }
    if style == Style::B {
        p.hair_back = p.hair;
        p.navy_shade = p.navy;
        p.white_shade = p.white;
        p.card_shade = p.card;
        p.skin_shade = p.skin;
    }
    p
}

/// 頭（A 座標のまま縮尺・傾き・位置を変える）と体（y を区間線形、x を倍率）を別々に配置する。
struct Lay {
    k: f32,
    ang: f32,
    hdx: f32,
    ys: [f32; 6],
    wx: f32,
}

const KA: [f32; 6] = [340.0, 384.0, 520.0, 700.0, 900.0, 960.0];

impl Lay {
    fn new(who: Who, style: Style, opts: &Opts) -> Lay {
        let (ang, hdx) = match who {
            Who::Hinata => (-0.06, -6.0),
            Who::Shiori => (0.05, 5.0),
        };
        let ang = opts.head_ang.unwrap_or(ang);
        match style {
            Style::A | Style::B => Lay {
                k: 1.0,
                ang,
                hdx,
                ys: KA,
                wx: 1.0,
            },
            Style::C => Lay {
                k: 0.72,
                ang,
                hdx: hdx * 0.72,
                ys: [262.0, 296.0, 480.0, 730.0, 900.0, 1180.0],
                wx: 1.0,
            },
        }
    }

    fn head(&self, (x, y): P) -> P {
        let (dx, dy) = ((x - 250.0) * self.k, (y - 340.0) * self.k);
        let (s, c) = self.ang.sin_cos();
        (
            250.0 + self.hdx + dx * c - dy * s,
            self.ys[0] + dx * s + dy * c,
        )
    }

    fn ymap(&self, y: f32) -> f32 {
        let i = if y <= KA[0] {
            0
        } else {
            (0..5).find(|&i| y <= KA[i + 1]).unwrap_or(4)
        };
        let t = (y - KA[i]) / (KA[i + 1] - KA[i]);
        self.ys[i] + (self.ys[i + 1] - self.ys[i]) * t
    }

    fn body(&self, (x, y): P) -> P {
        (250.0 + (x - 250.0) * self.wx, self.ymap(y))
    }
}

/// `draw_base` が返す、表情描画用のクリップ類。
pub struct Parts {
    pub face: Mask,
    pub clip: Mask,
}

struct Pen<'a, 'b> {
    c: &'a mut Ctx<'b>,
    p: Pal,
    who: Who,
    style: Style,
    lay: Lay,
    opts: Opts,
}

impl<'a, 'b> Pen<'a, 'b> {
    fn new(c: &'a mut Ctx<'b>, who: Who, style: Style, opts: Opts) -> Self {
        Pen {
            p: pal(who, style),
            who,
            style,
            lay: Lay::new(who, style, &opts),
            opts,
            c,
        }
    }

    /// 切り紙風の縁の不揃い（低周波のゆらぎ）。
    fn wob(&self, (x, y): P) -> P {
        if self.style != Style::B {
            return (x, y);
        }
        (
            x + 2.2 * (0.045 * y + 0.021 * x + 0.8).sin() + 0.9 * (0.13 * y + 1.7).sin(),
            y + 2.2 * (0.05 * x + 0.027 * y + 2.0).sin() + 0.9 * (0.11 * x).sin(),
        )
    }

    fn hm(&self, p: &Path) -> Mask {
        let q = p.map(|x, y| self.lay.head(self.wob((x, y))));
        self.c.mask(&q)
    }

    /// 表情パーツ用（ゆらぎなし）。
    fn hf(&self, p: &Path) -> Mask {
        let q = p.map(|x, y| self.lay.head((x, y)));
        self.c.mask(&q)
    }

    fn bm(&self, p: &Path) -> Mask {
        let q = p.map(|x, y| self.lay.body(self.wob((x, y))));
        self.c.mask(&q)
    }

    /// べた塗り 1 枚。B は紙の重なりらしい 2px のベタ影を下に敷く。
    fn piece(&mut self, m: &Mask, color: Rgba32) {
        if self.style == Style::B {
            let sh = self.c.shift(m, 2.0, 3.0);
            self.c.fill(&sh, self.p.paper_shadow);
        }
        self.c.fill(m, color);
    }

    /// `shape` の内側（`clip` の範囲）に 1 段のベタ影を塗る。B は影を持たない。
    fn shade(&mut self, shape: &Mask, clip: &Mask, color: Rgba32) {
        if self.style == Style::B {
            return;
        }
        self.c.fill_clipped(shape, clip, color);
    }

    /// 右側（光源は左）に落ちる 1 段の影。
    fn side_shade(&mut self, m: &Mask, dx: f32, color: Rgba32) {
        if self.style == Style::B {
            return;
        }
        let moved = self.c.shift(m, -dx, 0.0);
        let cres = m.sub(&moved);
        self.c.fill(&cres, color);
    }

    // ---------------- 髪 ----------------

    fn back_hair(&mut self) {
        let pts: Vec<P> = match self.who {
            Who::Hinata => vec![
                (250.0, 60.0),
                (338.0, 68.0),
                (394.0, 112.0),
                (416.0, 190.0),
                (421.0, 262.0),
                (414.0, 320.0),
                (388.0, 342.0),
                (352.0, 326.0),
                (310.0, 302.0),
                (250.0, 296.0),
                (190.0, 302.0),
                (160.0, 316.0),
                (126.0, 342.0),
                (92.0, 336.0),
                (78.0, 270.0),
                (82.0, 190.0),
                (106.0, 112.0),
                (162.0, 68.0),
            ],
            Who::Shiori => vec![
                (250.0, 58.0),
                (338.0, 66.0),
                (396.0, 110.0),
                (420.0, 190.0),
                (430.0, 300.0),
                (428.0, 460.0),
                (426.0, 600.0),
                (412.0, 640.0),
                (340.0, 646.0),
                (250.0, 640.0),
                (160.0, 648.0),
                (78.0, 640.0),
                (76.0, 600.0),
                (74.0, 460.0),
                (74.0, 300.0),
                (82.0, 190.0),
                (106.0, 112.0),
                (162.0, 66.0),
            ],
        };
        let m = self.hm(&Path::closed_spline(&pts));
        let c = self.p.hair_back;
        self.piece(&m, c);
    }

    fn front_hair(&mut self, face: &Mask) -> Mask {
        let (bangs, locks): (Vec<P>, Locks) = match self.who {
            Who::Hinata => {
                let mut bangs = vec![
                    (108.0, 210.0),
                    (110.0, 150.0),
                    (140.0, 100.0),
                    (196.0, 66.0),
                    (262.0, 58.0),
                    (330.0, 70.0),
                    (382.0, 112.0),
                    (404.0, 165.0),
                    (402.0, 208.0),
                ];
                // 下端（右 → 左）: 房の先端と切れ込みを交互に
                bangs.extend_from_slice(&[
                    (366.0, 200.0),
                    (356.0, 204.0),
                    (330.0, 176.0),
                    (310.0, 214.0),
                    (278.0, 172.0),
                    (254.0, 206.0),
                    (222.0, 176.0),
                    (198.0, 216.0),
                    (170.0, 180.0),
                    (146.0, 206.0),
                    (124.0, 202.0),
                ]);
                let locks = vec![
                    vec![
                        (116.0, 186.0, 32.0),
                        (112.0, 250.0, 26.0),
                        (122.0, 304.0, 16.0),
                    ],
                    vec![
                        (386.0, 186.0, 32.0),
                        (390.0, 246.0, 26.0),
                        (384.0, 292.0, 14.0),
                    ],
                ];
                (bangs, locks)
            }
            Who::Shiori => {
                let mut bangs = vec![
                    (106.0, 198.0),
                    (106.0, 140.0),
                    (136.0, 96.0),
                    (192.0, 66.0),
                    (256.0, 58.0),
                    (322.0, 66.0),
                    (376.0, 98.0),
                    (404.0, 142.0),
                    (404.0, 198.0),
                ];
                bangs.extend_from_slice(&[
                    (372.0, 196.0),
                    (330.0, 200.0),
                    (290.0, 194.0),
                    (250.0, 199.0),
                    (210.0, 193.0),
                    (170.0, 198.0),
                    (134.0, 195.0),
                ]);
                let locks = vec![
                    vec![
                        (122.0, 160.0, 34.0),
                        (116.0, 300.0, 36.0),
                        (120.0, 430.0, 36.0),
                        (130.0, 540.0, 30.0),
                    ],
                    vec![
                        (380.0, 160.0, 34.0),
                        (386.0, 300.0, 36.0),
                        (382.0, 420.0, 36.0),
                        (374.0, 500.0, 30.0),
                    ],
                ];
                (bangs, locks)
            }
        };
        let mut hair = self.hm(&Path::closed_spline(&bangs));
        for l in &locks {
            let lm = self.hm(&Path::tube(l));
            hair = hair.union(&lm);
        }
        let c = self.p.hair;
        // 前髪は顔の上に 1 枚として描く（B は紙の影つき）
        self.piece(&hair, c);
        // 髪留め
        if self.who == Who::Hinata {
            let c = self.p.clip;
            for (a, b) in [
                ((338.0, 104.0), (352.0, 146.0)),
                ((358.0, 100.0), (372.0, 142.0)),
            ] {
                let m = self.hf(&Path::stroke(&[(a.0, a.1, 11.0), (b.0, b.1, 11.0)]));
                self.c.fill(&m, c);
            }
        }
        let _ = face;
        hair
    }

    // ---------------- 顔・首 ----------------

    fn face_path() -> Path {
        Path::closed_spline(&[
            (250.0, 86.0),
            (332.0, 100.0),
            (380.0, 160.0),
            (392.0, 230.0),
            (366.0, 300.0),
            (310.0, 334.0),
            (250.0, 342.0),
            (190.0, 334.0),
            (134.0, 300.0),
            (108.0, 230.0),
            (120.0, 160.0),
            (168.0, 100.0),
        ])
    }

    fn neck_and_body_top(&mut self) -> Mask {
        let neck = self.bm(&Path::polygon(&[
            (224.0, 322.0),
            (276.0, 322.0),
            (280.0, 378.0),
            (220.0, 378.0),
        ]));
        let c = self.p.skin;
        self.c.fill(&neck, c);
        neck
    }

    // ---------------- 体 ----------------

    fn legs(&mut self) {
        let (lx, rx) = match self.who {
            Who::Hinata => (208.0, 292.0),
            Who::Shiori => (212.0, 288.0),
        };
        for (i, cx) in [lx, rx].into_iter().enumerate() {
            let leg = self.bm(&Path::polygon(&[
                (cx - 23.0, 680.0),
                (cx + 23.0, 680.0),
                (cx + 22.0, 950.0),
                (cx - 22.0, 950.0),
            ]));
            let c = self.p.skin;
            self.piece(&leg, c);
            // 白ソックス（足首側）
            let sock = self.bm(&Path::polygon(&[
                (cx - 23.0, 876.0),
                (cx + 23.0, 876.0),
                (cx + 22.0, 950.0),
                (cx - 22.0, 950.0),
            ]));
            let c = self.p.white;
            self.c.fill(&sock, c);
            let toe = if i == 0 { -6.0 } else { 6.0 };
            let shoe = self.bm(&Path::ellipse(cx + toe, 952.0, 34.0, 15.0, 0.0));
            let c = self.p.shoe;
            self.piece(&shoe, c);
        }
    }

    fn skirt(&mut self) {
        let mut p = Path::new();
        p.move_to(176.0, 508.0)
            .line_to(328.0, 508.0)
            .line_to(394.0, 698.0);
        p.spline_to(&[
            (334.0, 713.0),
            (252.0, 718.0),
            (170.0, 712.0),
            (108.0, 702.0),
        ]);
        let m = self.bm(&p);
        let c = self.p.navy;
        self.piece(&m, c);
        // ひだ 3 本（1 段の濃い影）
        let sc = self.p.navy_shade;
        for (x0, x1) in [(206.0, 142.0), (262.0, 258.0), (318.0, 372.0)] {
            let w = self.bm(&Path::polygon(&[
                (x0 - 8.0, 508.0),
                (x0 + 8.0, 508.0),
                (x1 + 20.0, 720.0),
                (x1 - 20.0, 720.0),
            ]));
            self.shade(&w, &m, sc);
        }
    }

    fn torso(&mut self) -> Mask {
        let t = Path::closed_spline(&[
            (212.0, 368.0),
            (250.0, 366.0),
            (288.0, 368.0),
            (330.0, 380.0),
            (348.0, 404.0),
            (344.0, 470.0),
            (334.0, 526.0),
            (250.0, 532.0),
            (166.0, 526.0),
            (156.0, 470.0),
            (152.0, 404.0),
            (170.0, 380.0),
        ]);
        let m = self.bm(&t);
        let base = if self.who == Who::Shiori {
            self.p.card
        } else {
            self.p.white
        };
        let shade = if self.who == Who::Shiori {
            self.p.card_shade
        } else {
            self.p.white_shade
        };
        self.piece(&m, base);
        self.side_shade(&m, 20.0, shade);
        m
    }

    fn collar_and_ribbon(&mut self) {
        // 胸元の素肌（V ネック）
        let v = self.bm(&Path::polygon(&[
            (226.0, 362.0),
            (274.0, 362.0),
            (250.0, 444.0),
        ]));
        let c = self.p.skin;
        self.c.fill(&v, c);
        if self.who == Who::Shiori {
            // カーディガンの下の白ブラウス
            let panel = self.bm(&Path::polygon(&[
                (236.0, 420.0),
                (264.0, 420.0),
                (286.0, 528.0),
                (214.0, 528.0),
            ]));
            let c = self.p.white;
            self.c.fill(&panel, c);
        }
        let navy = self.p.navy;
        let left = [
            (214.0, 362.0),
            (150.0, 402.0),
            (176.0, 458.0),
            (250.0, 484.0),
            (250.0, 442.0),
            (228.0, 366.0),
        ];
        let right: Vec<P> = left.iter().map(|&(x, y)| (500.0 - x, y)).collect();
        for w in [left.to_vec(), right] {
            let m = self.bm(&Path::polygon(&w));
            self.piece(&m, navy);
        }
        // リボン
        let (rc, rd) = (self.p.ribbon, self.p.ribbon_dark);
        let loop_l = Path::closed_spline(&[
            (238.0, 448.0),
            (206.0, 432.0),
            (184.0, 444.0),
            (186.0, 466.0),
            (208.0, 474.0),
            (238.0, 460.0),
        ]);
        let loop_r = Path::closed_spline(&[
            (262.0, 448.0),
            (294.0, 434.0),
            (318.0, 446.0),
            (316.0, 468.0),
            (292.0, 474.0),
            (262.0, 460.0),
        ]);
        let tail_l = Path::polygon(&[
            (242.0, 458.0),
            (226.0, 522.0),
            (244.0, 516.0),
            (256.0, 466.0),
        ]);
        let tail_r = Path::polygon(&[
            (258.0, 458.0),
            (282.0, 514.0),
            (264.0, 524.0),
            (246.0, 466.0),
        ]);
        for p in [tail_l, tail_r, loop_l, loop_r] {
            let m = self.bm(&p);
            self.piece(&m, rc);
        }
        let knot = self.bm(&Path::ellipse(250.0, 455.0, 15.0, 14.0, 0.0));
        self.c.fill(&knot, rd);
    }

    fn sleeve_and_arm(
        &mut self,
        sleeve: &[(f32, f32, f32)],
        skin: &[(f32, f32, f32)],
        hand: (f32, f32),
        sleeve_c: Rgba32,
    ) {
        let a = self.bm(&Path::tube(skin));
        let c = self.p.skin;
        self.piece(&a, c);
        let h = self.bm(&Path::ellipse(hand.0, hand.1, 20.0, 22.0, 0.0));
        self.piece(&h, c);
        let s = self.bm(&Path::tube(sleeve));
        self.piece(&s, sleeve_c);
    }

    fn arms(&mut self) {
        match self.who {
            Who::Hinata => {
                let w = self.p.white;
                // 下ろした腕（少し外へ開く）
                self.sleeve_and_arm(
                    &[(330.0, 394.0, 50.0), (346.0, 448.0, 46.0)],
                    &[
                        (346.0, 424.0, 30.0),
                        (354.0, 500.0, 28.0),
                        (352.0, 550.0, 26.0),
                    ],
                    (353.0, 568.0),
                    w,
                );
                // 上げた腕
                match self.opts.pose {
                    Pose::Stand => {
                        self.sleeve_and_arm(
                            &[(170.0, 394.0, 50.0), (128.0, 440.0, 46.0)],
                            &[
                                (134.0, 436.0, 30.0),
                                (82.0, 478.0, 29.0),
                                (54.0, 432.0, 27.0),
                                (48.0, 392.0, 26.0),
                            ],
                            (46.0, 368.0),
                            w,
                        );
                        // 親指
                        let th = self.bm(&Path::ellipse(66.0, 376.0, 8.0, 13.0, 0.5));
                        let c = self.p.skin;
                        self.piece(&th, c);
                    }
                    Pose::Sky => {
                        self.sleeve_and_arm(
                            &[(170.0, 394.0, 50.0), (136.0, 376.0, 46.0)],
                            &[
                                (140.0, 380.0, 30.0),
                                (98.0, 330.0, 28.0),
                                (60.0, 276.0, 26.0),
                                (36.0, 230.0, 26.0),
                            ],
                            (30.0, 214.0),
                            w,
                        );
                    }
                }
            }
            Who::Shiori => {
                let c = self.p.card;
                // 両手を前で組む（萌え袖で手は見えない）
                let sl = self.bm(&Path::tube(&[
                    (168.0, 396.0, 46.0),
                    (144.0, 470.0, 44.0),
                    (170.0, 540.0, 40.0),
                    (236.0, 574.0, 36.0),
                ]));
                let sr = self.bm(&Path::tube(&[
                    (332.0, 396.0, 46.0),
                    (356.0, 470.0, 44.0),
                    (332.0, 538.0, 40.0),
                    (268.0, 568.0, 36.0),
                ]));
                self.piece(&sl, c);
                self.piece(&sr, c);
            }
        }
    }

    // ---------------- 組み立て ----------------

    fn base(&mut self) -> Parts {
        self.back_hair();
        self.legs();
        self.skirt();
        let neck = self.neck_and_body_top();
        self.torso();
        self.collar_and_ribbon();
        self.arms();
        // 顔と首の影（顎の下 1 段）
        let face_m = self.hm(&Self::face_path());
        let under = self.c.shift(&face_m, 0.0, 22.0 * self.lay.k);
        let neck_area = neck.union(&self.bm(&Path::polygon(&[
            (226.0, 362.0),
            (274.0, 362.0),
            (250.0, 444.0),
        ])));
        let sh = self.p.skin_shade;
        self.shade(&under, &neck_area, sh);
        let s = self.p.skin;
        self.piece(&face_m, s);
        let hair = self.front_hair(&face_m);
        let clip = face_m.sub(&hair);
        Parts { face: face_m, clip }
    }

    // ---------------- 表情 ----------------

    fn eye_open(&mut self, clip: &Mask, cx: f32, cy: f32, hl: P) {
        let ec = self.p.eye;
        if self.style == Style::B {
            let m = self.hf(&Path::ellipse(cx, cy, 8.0, 8.0, 0.0)).mul(clip);
            self.c.fill(&m, ec);
            return;
        }
        let (rx, ry) = if self.who == Who::Shiori {
            (11.5, 16.5)
        } else {
            (12.5, 19.0)
        };
        let m = self.hf(&Path::ellipse(cx, cy, rx, ry, 0.0)).mul(clip);
        self.c.fill(&m, ec);
        let h = self
            .hf(&Path::ellipse(cx + hl.0, cy + hl.1, 4.0, 4.0, 0.0))
            .mul(clip);
        self.c.fill(&h, [1.0, 1.0, 1.0, 1.0]);
    }

    fn eye_happy(&mut self, clip: &Mask, cx: f32, cy: f32) {
        let ec = self.p.eye;
        let m = self
            .hf(&Path::stroke_spline(
                &[(cx - 17.0, cy + 7.0), (cx, cy - 8.0), (cx + 17.0, cy + 7.0)],
                |_| 6.0,
            ))
            .mul(clip);
        self.c.fill(&m, ec);
    }

    fn brow(&mut self, clip: &Mask, a: P, b: P) {
        let bc = self.p.brow;
        let m = self
            .hf(&Path::stroke(&[(a.0, a.1, 4.6), (b.0, b.1, 4.6)]))
            .mul(clip);
        self.c.fill(&m, bc);
    }

    fn arc(&mut self, clip: &Mask, pts: &[P], w: f32, color: Rgba32) {
        let m = self.hf(&Path::stroke_spline(pts, move |_| w)).mul(clip);
        self.c.fill(&m, color);
    }

    fn expr(&mut self, clip: &Mask, expr: Expr) {
        let (lx, rx, ey) = (196.0, 304.0, 262.0);
        let shiori = self.who == Who::Shiori;
        let b = self.style == Style::B;
        // 頬
        let (cc, crx, cry) = match expr {
            Expr::Blush => (self.p.blush, 31.0, 17.0),
            Expr::Sad => (self.p.skin, 0.0, 0.0),
            _ => (self.p.cheek, 26.0, 14.0),
        };
        if crx > 0.0 {
            for cx in [162.0, 338.0] {
                let m = self.hf(&Path::ellipse(cx, 296.0, crx, cry, 0.0)).mul(clip);
                self.c.fill(&m, cc);
            }
        }
        // 鼻（影のみ）
        if !b {
            let sh = self.p.skin_shade;
            self.arc(
                clip,
                &[(245.0, 277.0), (250.0, 281.0), (255.0, 277.0)],
                2.6,
                sh,
            );
        }
        let mc = self.p.mouth;
        match expr {
            Expr::Normal => {
                self.eye_open(clip, lx, ey, (3.5, -6.5));
                self.eye_open(clip, rx, ey, (3.5, -6.5));
                if shiori {
                    self.brow(clip, (176.0, 226.0), (214.0, 225.0));
                    self.brow(clip, (286.0, 225.0), (324.0, 226.0));
                    self.arc(
                        clip,
                        &[(241.0, 301.0), (250.0, 305.0), (259.0, 301.0)],
                        4.0,
                        mc,
                    );
                } else {
                    self.brow(clip, (176.0, 223.0), (214.0, 220.0));
                    self.brow(clip, (286.0, 220.0), (324.0, 223.0));
                    self.arc(
                        clip,
                        &[(233.0, 298.0), (250.0, 309.0), (267.0, 298.0)],
                        5.0,
                        mc,
                    );
                }
            }
            Expr::Smile => {
                self.eye_happy(clip, lx, ey);
                self.eye_happy(clip, rx, ey);
                if shiori {
                    self.brow(clip, (176.0, 224.0), (214.0, 221.0));
                    self.brow(clip, (286.0, 221.0), (324.0, 224.0));
                    self.arc(
                        clip,
                        &[(233.0, 297.0), (250.0, 309.0), (267.0, 297.0)],
                        4.6,
                        mc,
                    );
                } else {
                    self.brow(clip, (176.0, 216.0), (214.0, 212.0));
                    self.brow(clip, (286.0, 212.0), (324.0, 216.0));
                    let mut p = Path::new();
                    p.move_to(226.0, 292.0).line_to(274.0, 292.0);
                    p.spline_to(&[
                        (268.0, 310.0),
                        (250.0, 318.0),
                        (232.0, 310.0),
                        (226.0, 292.0),
                    ]);
                    let m = self.hf(&p).mul(clip);
                    let (mi, tg) = (self.p.mouth_in, self.p.tongue);
                    self.c.fill(&m, mi);
                    let t = self
                        .hf(&Path::ellipse(250.0, 311.0, 13.0, 6.0, 0.0))
                        .mul(&m);
                    if !b {
                        self.c.fill(&t, tg);
                    }
                }
            }
            Expr::Blush => {
                self.eye_open(clip, lx, ey + 2.0, (-3.5, 2.0));
                self.eye_open(clip, rx, ey + 2.0, (-3.5, 2.0));
                if shiori {
                    self.brow(clip, (176.0, 227.0), (214.0, 225.0));
                    self.brow(clip, (286.0, 225.0), (324.0, 227.0));
                } else {
                    self.brow(clip, (176.0, 226.0), (214.0, 222.0));
                    self.brow(clip, (286.0, 222.0), (324.0, 226.0));
                }
                self.arc(
                    clip,
                    &[
                        (236.0, 302.0),
                        (243.0, 306.0),
                        (250.0, 301.0),
                        (257.0, 306.0),
                        (264.0, 302.0),
                    ],
                    4.0,
                    mc,
                );
            }
            Expr::Sad => {
                self.eye_open(clip, lx, ey + 2.0, (3.5, -6.0));
                self.eye_open(clip, rx, ey + 2.0, (3.5, -6.0));
                self.brow(clip, (172.0, 232.0), (214.0, 218.0));
                self.brow(clip, (286.0, 218.0), (328.0, 232.0));
                self.arc(
                    clip,
                    &[(235.0, 308.0), (250.0, 299.0), (265.0, 308.0)],
                    4.6,
                    mc,
                );
            }
        }
    }
}

/// 体・髪・顔の素地を描く。戻り値は表情描画用クリップ。
pub fn draw_base(ctx: &mut Ctx, who: Who, style: Style, opts: Opts) -> Parts {
    Pen::new(ctx, who, style, opts).base()
}

/// 表情パーツ（眉・目・鼻・口・頬）を重ねる。
pub fn draw_expr(ctx: &mut Ctx, who: Who, style: Style, opts: Opts, expr: Expr, parts: &Parts) {
    Pen::new(ctx, who, style, opts).expr(&parts.clip, expr);
}

/// 立ち絵 4 枚（normal, smile, blush, sad）を 500x900 で生成する（既定の画風 A）。
pub fn render_sprites(who: Who) -> Vec<(Expr, image::RgbaImage)> {
    render_sprites_styled(who, Style::A)
}

pub fn render_sprites_styled(who: Who, style: Style) -> Vec<(Expr, image::RgbaImage)> {
    let mut base = Canvas::new(500, 900);
    let parts = {
        let mut ctx = Ctx::new(&mut base, Xf::identity());
        draw_base(&mut ctx, who, style, Opts::default())
    };
    [Expr::Normal, Expr::Smile, Expr::Blush, Expr::Sad]
        .iter()
        .map(|&e| {
            let mut cv = base.clone_canvas();
            {
                let mut ctx = Ctx::new(&mut cv, Xf::identity());
                draw_expr(&mut ctx, who, style, Opts::default(), e, &parts);
            }
            (e, cv.to_image(false))
        })
        .collect()
}
