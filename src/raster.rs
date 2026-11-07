//! Rendu d'une grille de cellules en pixels, à la manière d'une carte EGA :
//! le texte vient d'une police monospace, les bordures (═ ║ ╔ …), les blocs
//! (█ ▌ ▐) et les trames (░ ▒ ▓) sont tracés pixel par pixel pour se raccorder
//! parfaitement d'une cellule à l'autre.
use std::{collections::HashMap, process::Command};

use anyhow::{anyhow, Result};
use fontdue::{Font, FontSettings, Metrics};
use ratatui::{buffer::Cell, style::Color};

pub const BG: u32 = 0x000000;
const FG: u32 = 0xAAAAAA;

struct Glyph {
    m: Metrics,
    bmp: Vec<u8>,
}

pub struct Raster {
    fonts: Vec<Font>,
    pub px: f32,
    pub cw: u32,
    pub ch: u32,
    baseline: i32,
    cache: HashMap<char, Glyph>,
}

fn font_file(family: &str) -> Option<String> {
    let out = Command::new("fc-match")
        .args(["-f", "%{file}", family])
        .output()
        .ok()?;
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!p.is_empty()).then_some(p)
}

impl Raster {
    pub fn new(px: f32) -> Result<Self> {
        let mut paths: Vec<String> = Vec::new();
        for fam in ["DejaVu Sans Mono", "JetBrains Mono", "Noto Sans Mono", "DejaVu Sans", "Noto Sans Symbols2"] {
            if let Some(p) = font_file(fam) {
                if !paths.contains(&p) {
                    paths.push(p);
                }
            }
        }
        for p in [
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ] {
            if !paths.iter().any(|x| x == p) && std::path::Path::new(p).exists() {
                paths.push(p.to_string());
            }
        }
        let fonts: Vec<Font> = paths
            .iter()
            .filter_map(|p| std::fs::read(p).ok())
            .filter_map(|b| Font::from_bytes(b, FontSettings::default()).ok())
            .collect();
        if fonts.is_empty() {
            return Err(anyhow!(crate::i18n::t("aucune police monospace trouvée (installe fonts-dejavu-core)", "no monospace font found (install fonts-dejavu-core)")));
        }
        let mut r = Raster { fonts, px, cw: 0, ch: 0, baseline: 0, cache: HashMap::new() };
        r.set_px(px);
        Ok(r)
    }

    pub fn set_px(&mut self, px: f32) {
        self.px = px.clamp(8.0, 64.0);
        let f = &self.fonts[0];
        let (asc, desc) = f
            .horizontal_line_metrics(self.px)
            .map(|l| (l.ascent, l.descent))
            .unwrap_or((self.px * 0.93, -self.px * 0.24));
        self.ch = (asc - desc).ceil().max(8.0) as u32;
        self.baseline = asc.round() as i32;
        self.cw = f.metrics('M', self.px).advance_width.round().max(4.0) as u32;
        self.cache.clear();
    }

    fn glyph(&mut self, c: char) -> &Glyph {
        if !self.cache.contains_key(&c) {
            let font = self
                .fonts
                .iter()
                .find(|f| f.lookup_glyph_index(c) != 0)
                .unwrap_or(&self.fonts[0]);
            let c2 = if font.lookup_glyph_index(c) == 0 { '?' } else { c };
            let (m, bmp) = font.rasterize(c2, self.px);
            self.cache.insert(c, Glyph { m, bmp });
        }
        &self.cache[&c]
    }

    /// Dessine une cellule dans `fb` (largeur `stride`) au coin (x0, y0).
    pub fn draw_cell(&mut self, fb: &mut [u32], stride: usize, x0: usize, y0: usize, cell: &Cell) {
        let fg = rgb(cell.fg, FG);
        let bg = rgb(cell.bg, BG);
        let (cw, ch) = (self.cw as usize, self.ch as usize);
        let rows = fb.len() / stride.max(1);
        if x0 + cw > stride || y0 + ch > rows {
            return;
        }
        for y in 0..ch {
            let o = (y0 + y) * stride + x0;
            fb[o..o + cw].fill(bg);
        }
        let c = cell.symbol().chars().next().unwrap_or(' ');
        if c == ' ' {
            return;
        }
        let mut p = Painter { fb, stride, x0, y0, w: cw as i32, h: ch as i32, fg };
        if p.special(c) {
            return;
        }
        let baseline = self.baseline;
        let cw_f = self.cw as f32;
        let g = self.glyph(c);
        let off = ((cw_f - g.m.advance_width) / 2.0).round() as i32;
        let gx = x0 as i32 + off + g.m.xmin;
        let gy = y0 as i32 + baseline - g.m.ymin - g.m.height as i32;
        let (gw, gh) = (g.m.width as i32, g.m.height as i32);
        for j in 0..gh {
            let py = gy + j;
            if py < y0 as i32 || py >= (y0 + ch) as i32 {
                continue;
            }
            for i in 0..gw {
                let px = gx + i;
                if px < x0 as i32 || px >= (x0 + cw) as i32 {
                    continue;
                }
                let a = g.bmp[(j * gw + i) as usize] as u32;
                if a == 0 {
                    continue;
                }
                let idx = py as usize * stride + px as usize;
                fb[idx] = blend(bg, fg, a);
            }
        }
    }
}

fn blend(bg: u32, fg: u32, a: u32) -> u32 {
    let ch = |s: u32| {
        let b = (bg >> s) & 0xFF;
        let f = (fg >> s) & 0xFF;
        ((b * (255 - a) + f * a) / 255) << s
    };
    ch(16) | ch(8) | ch(0)
}

pub fn rgb(c: Color, default: u32) -> u32 {
    match c {
        Color::Rgb(r, g, b) => (r as u32) << 16 | (g as u32) << 8 | b as u32,
        Color::Black => 0x000000,
        Color::White => 0xFFFFFF,
        Color::Gray => 0xAAAAAA,
        Color::DarkGray => 0x555555,
        _ => default,
    }
}

struct Painter<'a> {
    fb: &'a mut [u32],
    stride: usize,
    x0: usize,
    y0: usize,
    w: i32,
    h: i32,
    fg: u32,
}

impl Painter<'_> {
    fn rect(&mut self, xa: i32, ya: i32, xb: i32, yb: i32) {
        let (xa, xb) = (xa.clamp(0, self.w), xb.clamp(0, self.w));
        let (ya, yb) = (ya.clamp(0, self.h), yb.clamp(0, self.h));
        for y in ya..yb {
            let o = (self.y0 + y as usize) * self.stride + self.x0;
            self.fb[o + xa as usize..o + xb as usize].fill(self.fg);
        }
    }

    /// Trame EGA : `on(gx, gy)` choisit les « gros pixels » allumés.
    fn dither(&mut self, on: impl Fn(usize, usize) -> bool) {
        let p = ((self.h / 16).max(1)) as usize;
        for y in 0..self.h as usize {
            let ay = self.y0 + y;
            let o = ay * self.stride + self.x0;
            for x in 0..self.w as usize {
                let ax = self.x0 + x;
                if on(ax / p, ay / p) {
                    self.fb[o + x] = self.fg;
                }
            }
        }
    }

    /// Caractères tracés à la main. Renvoie `false` si `c` doit venir de la police.
    fn special(&mut self, c: char) -> bool {
        let (w, h) = (self.w, self.h);
        let lw = ((h as f32 / 18.0).round() as i32).max(1);
        let (s, e) = (lw / 2, lw - lw / 2);
        let (cx, cy, d) = (w / 2, h / 2, lw);
        let hl = |p: &mut Self, xa: i32, xb: i32, yc: i32| p.rect(xa, yc - s, xb, yc - s + lw);
        let vl = |p: &mut Self, xc: i32, ya: i32, yb: i32| p.rect(xc - s, ya, xc - s + lw, yb);
        // traits simples : (haut, bas, gauche, droite)
        let single = match c {
            '─' => Some((false, false, true, true)),
            '│' => Some((true, true, false, false)),
            '├' => Some((true, true, false, true)),
            '┤' => Some((true, true, true, false)),
            '└' => Some((true, false, false, true)),
            '┘' => Some((true, false, true, false)),
            '┌' => Some((false, true, false, true)),
            '┐' => Some((false, true, true, false)),
            '┬' => Some((false, true, true, true)),
            '┴' => Some((true, false, true, true)),
            '┼' => Some((true, true, true, true)),
            _ => None,
        };
        if let Some((up, down, left, right)) = single {
            if up {
                vl(self, cx, 0, cy + e);
            }
            if down {
                vl(self, cx, cy - s, h);
            }
            if left {
                hl(self, 0, cx + e, cy);
            }
            if right {
                hl(self, cx - s, w, cy);
            }
            return true;
        }
        match c {
            '═' => {
                hl(self, 0, w, cy - d);
                hl(self, 0, w, cy + d);
            }
            '║' => {
                vl(self, cx - d, 0, h);
                vl(self, cx + d, 0, h);
            }
            '╔' => {
                hl(self, cx - d - s, w, cy - d);
                vl(self, cx - d, cy - d - s, h);
                hl(self, cx + d - s, w, cy + d);
                vl(self, cx + d, cy + d - s, h);
            }
            '╗' => {
                hl(self, 0, cx + d + e, cy - d);
                vl(self, cx + d, cy - d - s, h);
                hl(self, 0, cx - d + e, cy + d);
                vl(self, cx - d, cy + d - s, h);
            }
            '╚' => {
                vl(self, cx - d, 0, cy + d + e);
                hl(self, cx - d - s, w, cy + d);
                vl(self, cx + d, 0, cy - d + e);
                hl(self, cx + d - s, w, cy - d);
            }
            '╝' => {
                vl(self, cx + d, 0, cy + d + e);
                hl(self, 0, cx + d + e, cy + d);
                vl(self, cx - d, 0, cy - d + e);
                hl(self, 0, cx - d + e, cy - d);
            }
            '╠' => {
                vl(self, cx - d, 0, h);
                vl(self, cx + d, 0, cy - d + e);
                vl(self, cx + d, cy + d - s, h);
                hl(self, cx + d - s, w, cy - d);
                hl(self, cx + d - s, w, cy + d);
            }
            '╣' => {
                vl(self, cx + d, 0, h);
                vl(self, cx - d, 0, cy - d + e);
                vl(self, cx - d, cy + d - s, h);
                hl(self, 0, cx - d + e, cy - d);
                hl(self, 0, cx - d + e, cy + d);
            }
            '╦' => {
                hl(self, 0, w, cy - d);
                hl(self, 0, cx - d + e, cy + d);
                hl(self, cx + d - s, w, cy + d);
                vl(self, cx - d, cy + d - s, h);
                vl(self, cx + d, cy + d - s, h);
            }
            '╩' => {
                hl(self, 0, w, cy + d);
                hl(self, 0, cx - d + e, cy - d);
                hl(self, cx + d - s, w, cy - d);
                vl(self, cx - d, 0, cy - d + e);
                vl(self, cx + d, 0, cy - d + e);
            }
            '█' => self.rect(0, 0, w, h),
            '▌' => self.rect(0, 0, w / 2, h),
            '▐' => self.rect(w / 2, 0, w, h),
            '▀' => self.rect(0, 0, w, h / 2),
            '▄' => self.rect(0, h / 2, w, h),
            '▁' | '▂' | '▃' | '▅' | '▆' | '▇' => {
                let eighths = match c {
                    '▁' => 1,
                    '▂' => 2,
                    '▃' => 3,
                    '▅' => 5,
                    '▆' => 6,
                    _ => 7,
                };
                self.rect(0, h - h * eighths / 8, w, h)
            }
            '░' => self.dither(|x, y| x % 2 == 0 && y % 2 == 0),
            '▒' => self.dither(|x, y| (x + y) % 2 == 0),
            '▓' => self.dither(|x, y| !(x % 2 == 1 && y % 2 == 1)),
            _ => return false,
        }
        true
    }
}
