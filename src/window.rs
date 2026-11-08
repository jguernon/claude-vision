//! Fenêtres natives (Wayland ou X11) : la grille de caractères est rendue en
//! pixels par `raster` et affichée via softbuffer. Seules les cellules qui
//! changent d'une image à l'autre sont redessinées.
//!
//! Deux vues partagent le même moteur : la fenêtre principale et la MAP, qui
//! suit la session choisie dans la fenêtre principale.
use crate::i18n::{fr, t};
use std::{
    num::NonZeroU32,
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

use crate::{
    engine::Engine,
    map::{self, MapState},
    raster::{Raster, BG},
    ui,
    util::now_ms,
};

const APP_ID: &str = "claude-vision";
const FRAME: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ViewKind {
    Main,
    Map,
}

impl ViewKind {
    /// Nom de la fenêtre dans le fichier des zooms mémorisés.
    fn key(self) -> &'static str {
        match self {
            ViewKind::Main => "main",
            ViewKind::Map => "map",
        }
    }
    /// Taille de police par défaut (Ctrl 0 y revient).
    fn base_px(self) -> f32 {
        match self {
            ViewKind::Main => 14.0,
            ViewKind::Map => 12.0,
        }
    }
    fn start_size(self) -> (u32, u32) {
        match self {
            ViewKind::Main => (124, 36),
            ViewKind::Map => (150, 84),
        }
    }
    fn min_size(self) -> (u32, u32) {
        match self {
            ViewKind::Main => (80, 20),
            ViewKind::Map => (map::MIN_W as u32, map::MIN_H as u32),
        }
    }
}

/// `~/.config/claude-vision/zoom` : une ligne « main 14 » / « map 12 » par fenêtre.
fn zoom_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("claude-vision").join("zoom"))
}

fn read_zooms() -> Vec<(String, f32)> {
    zoom_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| {
            t.lines()
                .filter_map(|l| {
                    let mut it = l.split_whitespace();
                    Some((it.next()?.to_string(), it.next()?.parse::<f32>().ok()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Dernier zoom utilisé pour cette fenêtre, s'il a été mémorisé.
fn load_zoom(kind: ViewKind) -> Option<f32> {
    read_zooms()
        .into_iter()
        .find(|(k, _)| k == kind.key())
        .map(|(_, z)| z.clamp(8.0, 40.0))
}

/// Mémorise le zoom de cette fenêtre (les autres lignes sont gardées).
fn save_zoom(kind: ViewKind, zoom: f32) {
    let Some(path) = zoom_path() else { return };
    let mut all = read_zooms();
    all.retain(|(k, _)| k != kind.key());
    all.push((kind.key().to_string(), zoom));
    all.sort_by(|a, b| a.0.cmp(&b.0));
    let text: String = all.iter().map(|(k, z)| format!("{k} {z}\n")).collect();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

#[derive(Default, Clone, Copy)]
struct Grid {
    ox: u32,
    oy: u32,
    cols: u16,
    rows: u16,
}

struct View {
    kind: ViewKind,
    /// chaque fenêtre a sa propre taille de caractères
    raster: Raster,
    zoom: f32,
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    fb: Vec<u32>,
    fb_size: (u32, u32),
    cur: Buffer,
    prev: Buffer,
    prev_valid: bool,
    grid: Grid,
    cursor: (f64, f64),
    title: String,
}

pub fn run(engine: Engine, with_map: bool) -> Result<()> {
    let el = EventLoop::new()?;
    // vérifie tout de suite qu'une police est disponible
    Raster::new(ViewKind::Main.base_px())?;
    let mut w = Win {
        engine,
        scale: 1.0,
        context: None,
        views: Vec::new(),
        map: MapState::default(),
        mods: ModifiersState::empty(),
        next_frame: Instant::now(),
        with_map,
    };
    let res = el.run_app(&mut w);
    w.engine.shutdown();
    res?;
    Ok(())
}

struct Win {
    engine: Engine,
    scale: f64,
    context: Option<softbuffer::Context<Rc<Window>>>,
    views: Vec<View>,
    map: MapState,
    mods: ModifiersState,
    next_frame: Instant,
    with_map: bool,
}

impl Win {
    fn apply_zoom(&mut self, i: usize) {
        let scale = self.scale as f32;
        if let Some(v) = self.views.get_mut(i) {
            v.raster.set_px(v.zoom * scale);
            v.prev_valid = false;
            v.fb_size = (0, 0);
            v.window.request_redraw();
        }
    }

    fn redraw_all(&self) {
        for v in &self.views {
            v.window.request_redraw();
        }
    }

    fn has_map(&self) -> bool {
        self.views.iter().any(|v| v.kind == ViewKind::Map)
    }

    fn close_map(&mut self) {
        self.views.retain(|v| v.kind != ViewKind::Map);
    }

    fn open_view(&mut self, el: &ActiveEventLoop, kind: ViewKind) -> bool {
        let zoom = load_zoom(kind).unwrap_or_else(|| kind.base_px());
        let mut raster = match Raster::new(zoom * self.scale as f32) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{} {e}", t("Police indisponible :", "Font unavailable:"));
                return false;
            }
        };
        // tailles en points logiques : la cellule physique divisée par l'échelle
        let cw = raster.cw as f64 / self.scale;
        let ch = raster.ch as f64 / self.scale;
        let (c, r) = kind.start_size();
        let (mc, mr) = kind.min_size();
        let title = match kind {
            ViewKind::Main => "Claude Vision",
            ViewKind::Map => "Claude Vision — MAP",
        };
        let mut attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(c as f64 * cw + cw, r as f64 * ch + cw))
            .with_min_inner_size(LogicalSize::new(mc as f64 * cw + cw, mr as f64 * ch + cw));
        attrs = winit::platform::wayland::WindowAttributesExtWayland::with_name(attrs, APP_ID, "");
        attrs = winit::platform::x11::WindowAttributesExtX11::with_name(attrs, APP_ID, APP_ID);
        let window = match el.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("{} {e}", t("Impossible d'ouvrir la fenêtre :", "Cannot open the window:"));
                return false;
            }
        };
        if self.views.is_empty() {
            self.scale = window.scale_factor();
            if (self.scale - 1.0).abs() > f64::EPSILON {
                raster.set_px(zoom * self.scale as f32);
            }
        }
        if self.context.is_none() {
            match softbuffer::Context::new(window.clone()) {
                Ok(c) => self.context = Some(c),
                Err(e) => {
                    eprintln!("{} {e}", t("Affichage indisponible :", "Display unavailable:"));
                    return false;
                }
            }
        }
        let Some(context) = self.context.as_ref() else { return false };
        let surface = match softbuffer::Surface::new(context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{} {e}", t("Surface indisponible :", "Surface unavailable:"));
                return false;
            }
        };
        window.request_redraw();
        self.views.push(View {
            kind,
            raster,
            zoom,
            window,
            surface,
            fb: Vec::new(),
            fb_size: (0, 0),
            cur: Buffer::empty(Rect::default()),
            prev: Buffer::empty(Rect::default()),
            prev_valid: false,
            grid: Grid::default(),
            cursor: (0.0, 0.0),
            title: String::new(),
        });
        true
    }

    fn render(&mut self, i: usize) {
        let tick = self.engine.tick;
        let now = now_ms();
        let Win { views, engine, map: map_state, .. } = self;
        let Some(v) = views.get_mut(i) else { return };
        let size = v.window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let (cw, ch) = (v.raster.cw, v.raster.ch);
        let pad = cw / 2;
        let cols = (w.saturating_sub(2 * pad) / cw).clamp(1, 1000) as u16;
        let rows = (h.saturating_sub(2 * pad) / ch).clamp(1, 1000) as u16;
        let ox = (w - cols as u32 * cw) / 2;
        let oy = (h - rows as u32 * ch) / 2;
        let mut force = false;
        if v.fb_size != (w, h) {
            v.fb = vec![BG; (w * h) as usize];
            v.fb_size = (w, h);
            v.prev_valid = false;
            force = true;
        }
        let area = Rect::new(0, 0, cols, rows);
        if v.cur.area != area {
            v.cur = Buffer::empty(area);
            v.prev = Buffer::empty(area);
            v.prev_valid = false;
            v.fb.fill(BG);
            force = true;
        }
        v.grid = Grid { ox, oy, cols, rows };
        v.cur.reset();
        match v.kind {
            ViewKind::Main => ui::draw(&mut v.cur, area, &engine.app, tick),
            ViewKind::Map => map::draw(&mut v.cur, area, &engine.app, map_state, now, tick),
        }
        let mut changed = 0usize;
        for y in 0..rows {
            for x in 0..cols {
                let Some(c) = v.cur.cell((x, y)) else { continue };
                if v.prev_valid && v.prev.cell((x, y)) == Some(c) {
                    continue;
                }
                changed += 1;
                v.raster.draw_cell(&mut v.fb, w as usize, (ox + x as u32 * cw) as usize, (oy + y as u32 * ch) as usize, c);
            }
        }
        std::mem::swap(&mut v.cur, &mut v.prev);
        v.prev_valid = true;

        if changed > 0 || force {
            if let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
                if v.surface.resize(nw, nh).is_ok() {
                    if let Ok(mut buf) = v.surface.buffer_mut() {
                        if buf.len() == v.fb.len() {
                            buf.copy_from_slice(&v.fb);
                            let _ = buf.present();
                        }
                    }
                }
            }
        }

        let app = &engine.app;
        let title = match v.kind {
            ViewKind::Main => format!(
                "Claude Vision — {} session{} · {} active{}",
                app.sessions.len(),
                if app.sessions.len() > 1 { "s" } else { "" },
                app.busy_count(),
                if app.busy_count() > 1 && fr() { "s" } else { "" }
            ),
            ViewKind::Map => match app.sessions.get(app.selected) {
                Some(s) => format!("Claude Vision — MAP · {}", s.info.name),
                None => "Claude Vision — MAP".to_string(),
            },
        };
        if title != v.title {
            v.window.set_title(&title);
            v.title = title;
        }
    }

    fn cell_at(&self, i: usize) -> Option<(u16, u16)> {
        let v = self.views.get(i)?;
        let g = v.grid;
        let (x, y) = v.cursor;
        if x < g.ox as f64 || y < g.oy as f64 {
            return None;
        }
        let col = ((x - g.ox as f64) / v.raster.cw as f64) as u16;
        let row = ((y - g.oy as f64) / v.raster.ch as f64) as u16;
        (col < g.cols && row < g.rows).then_some((col, row))
    }

    /// Renvoie vrai si un clic a choisi une session dans la fenêtre principale.
    fn mouse(&mut self, i: usize, kind: MouseEventKind) -> bool {
        let Some(v) = self.views.get(i) else { return false };
        let (view_kind, cols, rows) = (v.kind, v.grid.cols, v.grid.rows);
        let clicked = match view_kind {
            ViewKind::Main => {
                let Some((column, row)) = self.cell_at(i) else { return false };
                let m = MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
                self.engine.app.on_mouse(m, cols, rows)
            }
            ViewKind::Map => {
                // la molette change aussi de session depuis la carte
                match kind {
                    MouseEventKind::ScrollUp => self.engine.app.on_key(KeyCode::Up, KeyModifiers::NONE),
                    MouseEventKind::ScrollDown => self.engine.app.on_key(KeyCode::Down, KeyModifiers::NONE),
                    _ => false,
                };
                false
            }
        };
        self.redraw_all();
        clicked
    }

    fn key(&mut self, el: &ActiveEventLoop, i: usize, key: &Key) {
        let in_map = self.views.get(i).map(|v| v.kind == ViewKind::Map).unwrap_or(false);
        let ctrl = self.mods.control_key();
        if ctrl {
            if let Key::Character(s) = key {
                let step = match s.as_str() {
                    "=" | "+" => Some(1.0),
                    "-" => Some(-1.0),
                    "0" => Some(0.0),
                    _ => None,
                };
                if let Some(step) = step {
                    // le zoom ne touche que la fenêtre où la touche a été pressée
                    if let Some(v) = self.views.get_mut(i) {
                        v.zoom = if step == 0.0 { v.kind.base_px() } else { (v.zoom + step).clamp(8.0, 40.0) };
                        save_zoom(v.kind, v.zoom);
                    }
                    self.apply_zoom(i);
                    return;
                }
            }
        }
        let code = match key {
            Key::Named(NamedKey::ArrowUp) => KeyCode::Up,
            Key::Named(NamedKey::ArrowDown) => KeyCode::Down,
            Key::Named(NamedKey::Enter) => KeyCode::Enter,
            Key::Named(NamedKey::Escape) => KeyCode::Esc,
            Key::Character(s) => match s.as_str().to_lowercase().chars().next() {
                Some(c) => KeyCode::Char(c),
                None => return,
            },
            _ => return,
        };
        // touches propres à la carte
        if code == KeyCode::Char('m') && !ctrl {
            if self.has_map() {
                self.close_map();
            } else {
                self.open_view(el, ViewKind::Map);
            }
            return;
        }
        if in_map && !ctrl && matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
            self.close_map();
            return;
        }
        if in_map && matches!(code, KeyCode::Enter) {
            return;
        }
        let km = if ctrl { KeyModifiers::CONTROL } else { KeyModifiers::NONE };
        if self.engine.app.on_key(code, km) {
            el.exit();
            return;
        }
        self.redraw_all();
    }
}

impl ApplicationHandler for Win {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.views.is_empty() {
            if !self.open_view(el, ViewKind::Main) {
                el.exit();
                return;
            }
            if self.with_map {
                self.open_view(el, ViewKind::Map);
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(i) = self.views.iter().position(|v| v.window.id() == id) else { return };
        let kind = self.views[i].kind;
        match event {
            WindowEvent::CloseRequested => {
                if kind == ViewKind::Main {
                    el.exit();
                } else {
                    self.views.remove(i);
                }
            }
            WindowEvent::Resized(_) => self.views[i].window.request_redraw(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor;
                for k in 0..self.views.len() {
                    self.apply_zoom(k);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    let key = event.logical_key.clone();
                    self.key(el, i, &key);
                }
            }
            WindowEvent::CursorMoved { position, .. } => self.views[i].cursor = (position.x, position.y),
            WindowEvent::MouseInput { state: ElementState::Pressed, button: winit::event::MouseButton::Left, .. } => {
                // un clic sur une session ouvre la carte (ou la fait suivre cette session)
                if self.mouse(i, MouseEventKind::Down(MouseButton::Left)) && !self.has_map() {
                    self.open_view(el, ViewKind::Map);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y,
                };
                if dy > 0.0 {
                    self.mouse(i, MouseEventKind::ScrollUp);
                } else if dy < 0.0 {
                    self.mouse(i, MouseEventKind::ScrollDown);
                }
            }
            WindowEvent::RedrawRequested => self.render(i),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.engine.update();
        let now = Instant::now();
        if now >= self.next_frame {
            self.engine.tick += 1;
            self.next_frame = now + FRAME;
            self.redraw_all();
        }
        el.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
    }
}

fn render_png(buf: &Buffer, path: &str, px: f32) -> Result<()> {
    let mut raster = Raster::new(px)?;
    let (cw, ch) = (raster.cw, raster.ch);
    let (cols, rows) = (buf.area.width, buf.area.height);
    let pad = cw / 2;
    let (w, h) = (cols as u32 * cw + 2 * pad, rows as u32 * ch + 2 * pad);
    let mut fb = vec![BG; (w * h) as usize];
    for y in 0..rows {
        for x in 0..cols {
            if let Some(c) = buf.cell((x, y)) {
                raster.draw_cell(&mut fb, w as usize, (pad + x as u32 * cw) as usize, (pad + y as u32 * ch) as usize, c);
            }
        }
    }
    let mut rgb = Vec::with_capacity(fb.len() * 3);
    for p in &fb {
        rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgb)?;
    println!("{path} : {w}×{h} px, cellule {cw}×{ch}");
    Ok(())
}

/// Rend une image PNG de l'écran principal (sans fenêtre), pour vérifier le rendu.
pub fn snapshot(engine: &Engine, path: &str, cols: u16, rows: u16) -> Result<()> {
    let area = Rect::new(0, 0, cols, rows);
    let mut buf = Buffer::empty(area);
    ui::draw(&mut buf, area, &engine.app, engine.tick + 6);
    render_png(&buf, path, ViewKind::Main.base_px())
}

/// Rend une image PNG de la MAP de la session choisie, avec quelques paquets en vol.
pub fn snapshot_map(engine: &Engine, path: &str, cols: u16, rows: u16) -> Result<()> {
    let area = Rect::new(0, 0, cols, rows);
    let mut buf = Buffer::empty(area);
    let mut ms = MapState::default();
    ms.preload(&engine.app, 8);
    let now = now_ms();
    map::draw(&mut buf, area, &engine.app, &mut ms, now, engine.tick);
    // une image prise au milieu du trajet des paquets
    let mut buf = Buffer::empty(area);
    map::draw(&mut buf, area, &engine.app, &mut ms, now + 450, engine.tick + 7);
    render_png(&buf, path, ViewKind::Map.base_px())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_is_remembered_per_window() {
        let dir = std::env::temp_dir().join(format!("claude-vision-zoom-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        assert_eq!(load_zoom(ViewKind::Main), None);
        save_zoom(ViewKind::Main, 13.0);
        save_zoom(ViewKind::Map, 11.0);
        save_zoom(ViewKind::Main, 15.0);
        assert_eq!(load_zoom(ViewKind::Main), Some(15.0));
        assert_eq!(load_zoom(ViewKind::Map), Some(11.0));
        let text = std::fs::read_to_string(dir.join("claude-vision").join("zoom")).unwrap();
        assert_eq!(text, "main 15\nmap 11\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
