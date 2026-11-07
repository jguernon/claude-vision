//! Vue MAP : carte animée d'une session, dans le style ASCII EGA de
//! claude-vision. L'humain et les outils de part et d'autre de la session
//! principale, les agents délégués en dessous, puis le retour vers la session.
//! Chaque événement du transcript devient un paquet `●` qui voyage le long
//! des liaisons.
use crate::i18n::{fr, t};
use std::{cmp::Reverse, collections::HashMap};

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::{
    ega::*,
    state::{AgentState, AgentStatus, App, FlowEntry, SessionState},
    transcript::{Dir, Kind},
    ui::{agent_rank, hline, seg, st, stb, Cv, Seg},
    util::*,
};

const PACKET_MS: i64 = 900;
const FLASH_MS: i64 = 400;
const SPIN: [&str; 4] = ["|", "/", "-", "\\"];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum PathId {
    HumanOut,
    HumanIn,
    ToolsOut,
    ToolsIn,
    Down(usize),
    Up(usize),
}

struct Packet {
    path: PathId,
    start_ms: i64,
    color: Color,
}

/// État propre à la fenêtre MAP : session suivie, paquets en vol, cases d'agents.
#[derive(Default)]
pub struct MapState {
    session_id: Option<String>,
    last_seq: u64,
    packets: Vec<Packet>,
    slots: Vec<Option<String>>,
    agent_flash: HashMap<String, i64>,
    session_flash: i64,
    next_free: HashMap<PathId, i64>,
}

impl MapState {
    /// Rejoue les `n` derniers événements de la session choisie (aperçu `--snapshot-map`).
    pub fn preload(&mut self, app: &App, n: usize) {
        let Some(s) = app.sessions.get(app.selected) else { return };
        self.session_id = Some(s.info.id.clone());
        let seqs: Vec<u64> = app
            .flow
            .iter()
            .filter(|e| e.session_id == s.info.id && e.agent.is_none())
            .map(|e| e.seq)
            .collect();
        self.last_seq = seqs.iter().rev().nth(n).copied().unwrap_or(0);
    }

    fn launch(&mut self, path: PathId, color: Color, now: i64) {
        let start = self.next_free.get(&path).copied().unwrap_or(0).max(now);
        self.next_free.insert(path, start + 170);
        self.packets.push(Packet { path, start_ms: start, color });
        if self.packets.len() > 80 {
            self.packets.remove(0);
        }
    }

    fn slot_of(&self, s: &SessionState, link: Option<&str>) -> Option<usize> {
        let link = link?;
        let a = s.agents.iter().find(|a| a.tool_use_id == link)?;
        self.slots.iter().position(|x| x.as_deref() == Some(a.id.as_str()))
    }

    fn ingest(&mut self, app: &App, s: &SessionState, now: i64) {
        if self.session_id.as_deref() != Some(s.info.id.as_str()) {
            // nouvelle session suivie : on part du présent, sans rejouer l'historique
            *self = MapState::default();
            self.session_id = Some(s.info.id.clone());
            self.last_seq = app.flow_seq;
            return;
        }
        let new: Vec<&FlowEntry> = app
            .flow
            .iter()
            .filter(|e| e.seq > self.last_seq && e.session_id == s.info.id)
            .collect();
        // en rattrapage, on ne lance que les 24 derniers paquets de la session principale
        let main_total = new.iter().filter(|e| e.agent.is_none()).count();
        let mut main_seen = 0usize;
        let center = self.slots.len() / 2;
        for e in new {
            if let Some(aid) = &e.agent {
                self.agent_flash.insert(aid.clone(), now);
                continue;
            }
            main_seen += 1;
            if main_total - main_seen >= 24 {
                continue;
            }
            match e.ev.kind {
                Kind::Prompt => self.launch(PathId::HumanOut, YELLOW, now),
                Kind::Text => {
                    self.session_flash = now;
                    self.launch(PathId::HumanIn, WHITE, now)
                }
                Kind::Think => self.session_flash = now,
                Kind::Tool => self.launch(PathId::ToolsOut, LCYAN, now),
                Kind::Result => self.launch(PathId::ToolsIn, LMAGENTA, now),
                Kind::Error => self.launch(PathId::ToolsIn, LRED, now),
                Kind::Agent => {
                    let i = self.slot_of(s, e.ev.link.as_deref()).unwrap_or(center);
                    self.launch(PathId::Down(i), LCYAN, now)
                }
                Kind::Notif => {
                    let i = self.slot_of(s, e.ev.link.as_deref()).unwrap_or(center);
                    self.launch(PathId::Up(i), LGREEN, now)
                }
                Kind::System => {}
            }
        }
        self.last_seq = app.flow_seq;
        self.packets.retain(|p| now - p.start_ms < PACKET_MS + 100);
        self.agent_flash.retain(|_, t| now - *t < FLASH_MS);
    }

    /// Garde chaque agent dans sa case tant qu'il reste pertinent : les cases ne sautent pas.
    fn assign_slots(&mut self, s: &SessionState, n: usize, now: i64) {
        self.slots.resize(n, None);
        let mut ranked: Vec<&AgentState> = s.agents.iter().collect();
        ranked.sort_by_key(|a| (agent_rank(a, now), Reverse(a.last_activity_ms)));
        let desired: Vec<&str> = ranked.iter().take(n).map(|a| a.id.as_str()).collect();
        for slot in self.slots.iter_mut() {
            if let Some(id) = slot {
                if !desired.contains(&id.as_str()) {
                    *slot = None;
                }
            }
        }
        for id in desired {
            if self.slots.iter().any(|x| x.as_deref() == Some(id)) {
                continue;
            }
            if let Some(empty) = self.slots.iter_mut().find(|x| x.is_none()) {
                *empty = Some(id.to_string());
            }
        }
    }
}

// ── géométrie ────────────────────────────────────────────────────────────
//
// De haut en bas : HUMAIN ⇄ SESSION ⇄ OUTILS, puis une grille d'agents sur
// autant de rangées que la hauteur le permet. La délégation descend par un
// tronc à gauche de la grille, les retours par un tronc à droite, jusqu'à
// « RETOUR À LA SESSION » sous la grille.

struct Row {
    bus_y: u16,
    rbus_y: u16,
}

struct Layout {
    w: u16,
    journal_y: u16,
    footer_y: u16,
    human: Rect,
    models: Rect,
    files: Option<Rect>,
    tools: Rect,
    last: Rect,
    swarm: Option<Rect>,
    sess: Rect,
    cols: usize,
    rows: Vec<Row>,
    agents: Vec<Rect>,
    ltrunk: u16,
    rtrunk: u16,
    merge: Rect,
    merge_row: u16,
    r_out: u16,
    r_in: u16,
    cc: u16,
}

pub const MIN_W: u16 = 100;
pub const MIN_H: u16 = 35;

fn layout(w: u16, h: u16, n_agents: usize) -> Option<Layout> {
    if w < MIN_W || h < MIN_H {
        return None;
    }
    let tall = h >= 60;
    let footer_y = h - 1;
    let journal_rows = ((h as i32 - 38) / 3).clamp(5, 14) as u16;
    let journal_y = footer_y - journal_rows - 1;
    let y0: u16 = 4;
    let side_w: u16 = if w >= 140 { 30 } else if w >= 120 { 27 } else if w >= 110 { 26 } else { 24 };
    let left_x: u16 = 2;
    let right_x = w - 2 - side_w;
    let cx0 = left_x + side_w + 5;
    let cx1 = right_x - 5;
    let center_w = cx1 - cx0;
    let cc = cx0 + center_w / 2;
    let sess_w = center_w.min(40);
    let sess = Rect::new(cc - sess_w / 2, y0, sess_w, 6);

    // grille d'agents
    let gap: u16 = 2;
    let grid_w = center_w - 6;
    let cols = ((grid_w + gap) / (18 + gap)).clamp(1, 4);
    let box_w = ((grid_w - gap * (cols - 1)) / cols).min(30);
    let total = box_w * cols + gap * (cols - 1);
    let gx0 = cc - total / 2;
    let mut box_h: u16 = if tall { 9 } else { 7 };
    let bus0 = sess.y + sess.height + 2;
    let max_rows = (journal_y.saturating_sub(bus0 + 5) / (box_h + 5)).max(1);
    let needed = n_agents.max(1).div_ceil(cols as usize) as u16;
    let nrows = needed.clamp(1, max_rows);
    // l'espace qui reste sous le retour agrandit les cases (jusqu'à 4 lignes de plus)
    let used = bus0 + nrows * (box_h + 5) + 5;
    box_h += (journal_y.saturating_sub(used) / nrows).min(4);
    let mut rows = Vec::new();
    let mut agents = Vec::new();
    let mut y = bus0;
    for _ in 0..nrows {
        let box_y = y + 2;
        for c in 0..cols {
            agents.push(Rect::new(gx0 + c * (box_w + gap), box_y, box_w, box_h));
        }
        let rbus_y = box_y + box_h + 1;
        rows.push(Row { bus_y: y, rbus_y });
        y = rbus_y + 2;
    }
    let last_rbus = rows.last().map(|r| r.rbus_y).unwrap_or(bus0);
    let merge_w = center_w.min(46);
    let merge = Rect::new(cc - merge_w / 2, last_rbus + 2, merge_w, 4);

    // colonnes latérales
    let human = Rect::new(left_x, y0, side_w, if tall { 14 } else { 9 });
    let models = Rect::new(left_x, human.y + human.height + 1, side_w, 7);
    let tools = Rect::new(right_x, y0, side_w, if tall { 16 } else { 11 });
    let last = Rect::new(right_x, tools.y + tools.height + 1, side_w, if tall { 9 } else { 7 });
    let below = |top: u16| {
        let hgt = journal_y.saturating_sub(top + 1).min(24);
        (hgt >= 5).then(|| hgt)
    };
    let files = below(models.y + models.height + 1).map(|hh| Rect::new(left_x, models.y + models.height + 1, side_w, hh));
    let swarm = below(last.y + last.height + 1).map(|hh| Rect::new(right_x, last.y + last.height + 1, side_w, hh));

    Some(Layout {
        w,
        journal_y,
        footer_y,
        human,
        models,
        files,
        tools,
        last,
        swarm,
        sess,
        cols: cols as usize,
        rows,
        agents,
        ltrunk: gx0 - 2,
        rtrunk: gx0 + total + 1,
        merge,
        merge_row: merge.y + 1,
        r_out: sess.y + 2,
        r_in: sess.y + 3,
        cc,
    })
}

fn slot_x(r: &Rect) -> u16 {
    r.x + r.width / 2
}

fn horiz(v: &mut Vec<(u16, u16)>, from: u16, to: u16, y: u16) {
    if from <= to {
        v.extend((from..=to).map(|x| (x, y)));
    } else {
        v.extend((to..=from).rev().map(|x| (x, y)));
    }
}

fn path(l: &Layout, id: PathId) -> Vec<(u16, u16)> {
    let mut v = Vec::new();
    match id {
        PathId::HumanOut => horiz(&mut v, l.human.x + l.human.width, l.sess.x - 1, l.r_out),
        PathId::HumanIn => horiz(&mut v, l.sess.x - 1, l.human.x + l.human.width, l.r_in),
        PathId::ToolsOut => horiz(&mut v, l.sess.x + l.sess.width, l.tools.x - 1, l.r_out),
        PathId::ToolsIn => horiz(&mut v, l.tools.x - 1, l.sess.x + l.sess.width, l.r_in),
        PathId::Down(i) | PathId::Up(i) => {
            if l.agents.is_empty() {
                return v;
            }
            let i = i.min(l.agents.len() - 1);
            let a = l.agents[i];
            let row = &l.rows[i / l.cols];
            let x = slot_x(&a);
            if let PathId::Down(_) = id {
                let bus0 = l.rows[0].bus_y;
                v.extend((l.sess.y + l.sess.height..bus0).map(|y| (l.cc, y)));
                if i / l.cols == 0 {
                    horiz(&mut v, l.cc, x, bus0);
                } else {
                    horiz(&mut v, l.cc, l.ltrunk, bus0);
                    v.extend((bus0 + 1..row.bus_y).map(|y| (l.ltrunk, y)));
                    horiz(&mut v, l.ltrunk, x, row.bus_y);
                }
                v.extend((row.bus_y + 1..a.y).map(|y| (x, y)));
            } else {
                v.extend((a.y + a.height..row.rbus_y).map(|y| (x, y)));
                horiz(&mut v, x, l.rtrunk, row.rbus_y);
                v.extend((row.rbus_y + 1..l.merge_row).map(|y| (l.rtrunk, y)));
                horiz(&mut v, l.rtrunk, l.merge.x + l.merge.width, l.merge_row);
            }
        }
    }
    v
}

// ── briques de dessin ────────────────────────────────────────────────────

fn frame_box(cv: &mut Cv, r: Rect, single: bool, color: Color, title: &str, title_color: Color) {
    let (hz, vt, tl, tr, bl, br) = if single {
        ("─", "│", "┌", "┐", "└", "┘")
    } else {
        ("═", "║", "╔", "╗", "╚", "╝")
    };
    let lim = r.x + r.width;
    let bottom = r.y + r.height - 1;
    for y in r.y..=bottom {
        cv.fill(r.x, y, r.width, " ", st(LGRAY), lim);
    }
    cv.fill(r.x, r.y, r.width, hz, st(color), lim);
    cv.fill(r.x, bottom, r.width, hz, st(color), lim);
    for y in r.y + 1..bottom {
        cv.put(r.x, y, vt, st(color), lim);
        cv.put(lim - 1, y, vt, st(color), lim);
    }
    cv.put(r.x, r.y, tl, st(color), lim);
    cv.put(lim - 1, r.y, tr, st(color), lim);
    cv.put(r.x, bottom, bl, st(color), lim);
    cv.put(lim - 1, bottom, br, st(color), lim);
    if !title.is_empty() {
        let x = cv.put(r.x + 1, r.y, " ", st(color), lim - 1);
        let x = cv.put(x, r.y, title, st(title_color), lim - 1);
        cv.put(x, r.y, " ", st(color), lim - 1);
    }
}

/// Écrit la ligne `row` (0 = première ligne intérieure) d'une boîte.
fn line(cv: &mut Cv, r: Rect, row: u16, segs: &[Seg]) {
    if row + 2 >= r.height {
        return;
    }
    cv.segs(r.x + 2, r.y + 1 + row, segs, r.x + r.width - 2);
}

fn inner_w(r: Rect) -> usize {
    r.width.saturating_sub(4) as usize
}

/// Coupe `s` en lignes de `w` colonnes au plus, sur `max` lignes (la dernière finit par « … »).
fn wrap(s: &str, w: usize, max: usize) -> Vec<String> {
    if w == 0 || max == 0 {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut overflow = false;
    for word in s.split_whitespace() {
        let need = if cur.is_empty() { width(word) } else { width(&cur) + 1 + width(word) };
        if need > w && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            if lines.len() == max {
                overflow = true;
                break;
            }
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !overflow && !cur.is_empty() {
        lines.push(cur);
    }
    for l in lines.iter_mut() {
        if width(l) > w {
            *l = trunc(l, w);
        }
    }
    if overflow {
        if let Some(last) = lines.last_mut() {
            let t = trunc(last, w.saturating_sub(1));
            *last = format!("{}…", t.trim_end_matches('…'));
        }
    }
    lines
}

fn t_seg(fr_text: &'static str, en_text: &'static str) -> Seg {
    seg(t(fr_text, en_text), LGRAY)
}

fn effort_level(e: &str) -> usize {
    match e {
        "low" => 1,
        "medium" => 2,
        "high" => 3,
        "xhigh" => 4,
        "max" => 5,
        _ => 0,
    }
}

fn effort_meter(e: Option<&str>) -> Vec<Seg> {
    let lvl = e.map(effort_level).unwrap_or(0);
    vec![
        seg("▓".repeat(lvl), YELLOW),
        seg("░".repeat(5 - lvl), DGRAY),
        seg(format!(" {}", e.unwrap_or("—")), LGRAY),
    ]
}

// ── dessin ───────────────────────────────────────────────────────────────

pub fn draw(buf: &mut Buffer, area: Rect, app: &App, ms: &mut MapState, now: i64, tick: u64) {
    let mut cv = Cv { buf, w: area.width, h: area.height };
    let (w, h) = (cv.w, cv.h);
    for y in 0..h {
        cv.fill(0, y, w, " ", st(LGRAY), w);
    }
    let n_agents = app.sessions.get(app.selected).map(|s| s.agents.len()).unwrap_or(0);
    let Some(l) = layout(w, h, n_agents) else {
        cv.put(1, 1, &if fr() { format!("Agrandis la fenêtre : minimum {MIN_W} × {MIN_H}") } else { format!("Enlarge the window: at least {MIN_W} × {MIN_H}") }, st(YELLOW), w);
        return;
    };
    let Some(s) = app.sessions.get(app.selected) else {
        header(&mut cv, None, now);
        frame(&mut cv, &l);
        cv.put(l.cc.saturating_sub(18), 12, t("aucune session Claude Code en cours", "no Claude Code session running"), st(DGRAY), w);
        return;
    };
    ms.ingest(app, s, now);
    ms.assign_slots(s, l.agents.len(), now);

    header(&mut cv, Some(s), now);
    frame(&mut cv, &l);
    connectors(&mut cv, &l, s, ms, now, tick);
    human_box(&mut cv, &l, s, now);
    models_box(&mut cv, &l, s);
    tools_box(&mut cv, &l, s);
    last_box(&mut cv, &l, s);
    if let Some(r) = l.files {
        files_box(&mut cv, r, s);
    }
    if let Some(r) = l.swarm {
        swarm_box(&mut cv, r, s, now);
    }
    session_box(&mut cv, &l, s, ms, now, tick);
    for (i, r) in l.agents.iter().enumerate() {
        let agent = ms.slots.get(i).and_then(|x| x.as_ref()).and_then(|id| s.agents.iter().find(|a| &a.id == id));
        agent_box(&mut cv, *r, agent, ms, now, tick);
    }
    merge_box(&mut cv, &l, s);
    packets(&mut cv, &l, ms, now);
    journal(&mut cv, &l, app, s);
}

fn header(cv: &mut Cv, s: Option<&SessionState>, now: i64) {
    let w = cv.w;
    let bg = BLUE;
    cv.fill(0, 0, w, " ", stb(WHITE, bg), w);
    let mut left: Vec<Seg> = vec![
        (" ▌".into(), stb(WHITE, bg)),
        ("MAP".into(), stb(YELLOW, bg)),
        ("▐ ".into(), stb(WHITE, bg)),
    ];
    let mut right: Vec<Seg> = Vec::new();
    if let Some(s) = s {
        left.push((s.info.short.clone(), stb(LCYAN, bg)));
        left.push((" · ".into(), stb(WHITE, bg)));
        left.push((s.info.name.clone(), stb(WHITE, bg)));
        let model = s.model.clone().unwrap_or_else(|| "—".into());
        right.push((model, stb(WHITE, bg)));
        right.push((" · ".into(), stb(WHITE, bg)));
        if s.info.busy {
            right.push(("BUSY".into(), stb(LGREEN, bg)));
            right.push((format!(" {} ", fmt_elapsed(now - s.info.started_at_ms)), stb(WHITE, bg)));
        } else {
            right.push(("IDLE ".into(), stb(LGRAY, bg)));
        }
    }
    let rw: usize = right.iter().map(|(t, _)| width(t)).sum();
    let lim = w.saturating_sub(rw as u16 + 2);
    cv.segs(0, 0, &left, lim);
    cv.segs(w.saturating_sub(rw as u16), 0, &right, w);
}

fn frame(cv: &mut Cv, l: &Layout) {
    let w = l.w;
    hline(cv, 1, '╔', '╗', &[(1, t("CARTE", "MAP"))], None);
    for y in 2..l.journal_y {
        cv.put(0, y, "║", st(LGRAY), w);
        cv.put(w - 1, y, "║", st(LGRAY), w);
    }
    let legend: Vec<Seg> = vec![
        seg("■", LMAGENTA), t_seg(" humain   ", " human    "),
        seg("■", YELLOW), seg(" session   ", LGRAY),
        seg("■", LGREEN), t_seg(" outils   ", " tools    "),
        seg("■", LCYAN), seg(" agents   ", LGRAY),
        seg("■", LBLUE), t_seg(" modèles        ", " models         "),
        seg("●", LCYAN), t_seg(" envoi   ", " out     "),
        seg("●", LMAGENTA), t_seg(" retour   ", " back     "),
        seg("●", YELLOW), seg(" prompt   ", LGRAY),
        seg("●", LGREEN), t_seg(" agent fini", " agent done"),
    ];
    cv.segs(2, 2, &legend, w - 1);
    hline(cv, l.journal_y, '╠', '╣', &[(1, t("JOURNAL", "LOG"))], None);
    for y in l.journal_y + 1..l.footer_y {
        cv.put(0, y, "║", st(LGRAY), w);
        cv.put(w - 1, y, "║", st(LGRAY), w);
    }
    let y = l.footer_y;
    cv.fill(0, y, w, "═", st(LGRAY), w);
    cv.put(0, y, "╚", st(LGRAY), w);
    cv.put(w - 1, y, "╝", st(LGRAY), w);
    let keys: Vec<Seg> = vec![
        seg("═ ", LGRAY),
        seg("m", YELLOW), seg(t(" ou ", " or "), DGRAY), seg("Esc", YELLOW), seg(t(" fermer la carte", " close the map"), LGRAY),
        seg(" · ", DGRAY),
        seg("↑↓", YELLOW), seg(" session", LGRAY),
        seg(" · ", DGRAY),
        seg(t("clic", "click"), YELLOW), seg(t(" sur une session dans la fenêtre principale", " a session in the main window"), LGRAY),
        seg(" · ", DGRAY),
        seg("Ctrl +/-", YELLOW), seg(" zoom ", LGRAY),
    ];
    cv.segs(1, y, &keys, w - 1);
}

fn connectors(cv: &mut Cv, l: &Layout, s: &SessionState, ms: &MapState, now: i64, tick: u64) {
    let w = l.w;
    let line_st = st(DGRAY);
    let head_st = st(LGRAY);
    // humain ⇄ session, session ⇄ outils
    for (out, inn) in [(PathId::HumanOut, PathId::HumanIn), (PathId::ToolsOut, PathId::ToolsIn)] {
        for (pid, head) in [(out, "▶"), (inn, "◀")] {
            let p = path(l, pid);
            for (i, (x, y)) in p.iter().enumerate() {
                let last = i + 1 == p.len();
                cv.put(*x, *y, if last { head } else { "─" }, if last { head_st } else { line_st }, w);
            }
        }
    }
    // session → première rangée
    let bus0 = l.rows[0].bus_y;
    for y in l.sess.y + l.sess.height..bus0 {
        cv.put(l.cc, y, "│", line_st, w);
    }
    let nrows = l.rows.len();
    for (r, row) in l.rows.iter().enumerate() {
        let boxes = &l.agents[r * l.cols..((r + 1) * l.cols).min(l.agents.len())];
        let xs: Vec<u16> = boxes.iter().map(slot_x).collect();
        let first = xs[0];
        let lastx = *xs.last().unwrap();
        // ligne de délégation au-dessus de la rangée
        let lo = if r == 0 { if nrows > 1 { l.ltrunk } else { first.min(l.cc) } } else { l.ltrunk };
        let hi = if r == 0 { lastx.max(l.cc) } else { lastx };
        for x in lo..=hi {
            cv.put(x, row.bus_y, "─", line_st, w);
        }
        for &x in &xs {
            let c = if lo == hi {
                "│"
            } else if r == 0 && x == l.cc {
                "┼"
            } else if x == hi {
                "┐"
            } else if x == lo {
                "┌"
            } else {
                "┬"
            };
            cv.put(x, row.bus_y, c, line_st, w);
            cv.put(x, row.bus_y + 1, "▼", head_st, w);
            cv.put(x, row.rbus_y - 1, "│", line_st, w);
        }
        if r == 0 && !xs.contains(&l.cc) {
            cv.put(l.cc, row.bus_y, "┴", line_st, w);
        }
        if nrows > 1 {
            let c = if r == 0 { "┌" } else if r + 1 == nrows { "└" } else { "├" };
            cv.put(l.ltrunk, row.bus_y, c, line_st, w);
        }
        // ligne de retour sous la rangée
        for x in first..=l.rtrunk {
            cv.put(x, row.rbus_y, "─", line_st, w);
        }
        for &x in &xs {
            cv.put(x, row.rbus_y, if x == first { "└" } else { "┴" }, line_st, w);
        }
        cv.put(l.rtrunk, row.rbus_y, if r == 0 { "┐" } else { "┤" }, line_st, w);
    }
    // troncs
    let bus_rows: Vec<u16> = l.rows.iter().map(|r| r.bus_y).collect();
    let rbus_rows: Vec<u16> = l.rows.iter().map(|r| r.rbus_y).collect();
    if nrows > 1 {
        for y in bus0 + 1..*bus_rows.last().unwrap() {
            if !bus_rows.contains(&y) {
                cv.put(l.ltrunk, y, "│", line_st, w);
            }
        }
    }
    for y in rbus_rows[0] + 1..l.merge_row {
        if !rbus_rows.contains(&y) {
            cv.put(l.rtrunk, y, "│", line_st, w);
        }
    }
    cv.put(l.rtrunk, l.merge_row, "┘", line_st, w);
    let mr = l.merge.x + l.merge.width;
    for x in mr + 1..l.rtrunk {
        cv.put(x, l.merge_row, "─", line_st, w);
    }
    cv.put(mr, l.merge_row, "◀", head_st, w);

    // étiquette de délégation
    let active = s.agents.iter().filter(|a| agent_rank(a, now) < 2).count();
    let shown_active = ms
        .slots
        .iter()
        .flatten()
        .filter(|id| s.agents.iter().any(|a| &a.id == *id && agent_rank(a, now) < 2))
        .count();
    let mut label: Vec<Seg> = vec![
        seg(t("délégation · ", "delegation · "), CYAN),
        seg(if fr() { format!("{active} actif{}", if active > 1 { "s" } else { "" }) } else { format!("{active} active") }, if active > 0 { LGREEN } else { DGRAY }),
        seg(format!(" / {}", s.agents.len()), DGRAY),
    ];
    if active > shown_active {
        label.push(seg(format!(" · +{} {}", active - shown_active, t("hors carte", "off map")), BROWN));
    }
    cv.segs(l.cc + 2, bus0 - 1, &label, l.tools.x.saturating_sub(1).max(l.cc + 3));

    // points qui défilent sur les liaisons actives
    let mut ambient: Vec<(PathId, Color)> = Vec::new();
    for (i, id) in ms.slots.iter().enumerate() {
        let Some(id) = id else { continue };
        if let Some(a) = s.agents.iter().find(|a| &a.id == id) {
            if a.status(now) == AgentStatus::Working {
                ambient.push((PathId::Down(i), CYAN));
                ambient.push((PathId::Up(i), CYAN));
            }
        }
    }
    if s.info.busy {
        ambient.push((PathId::ToolsOut, GREEN));
        ambient.push((PathId::ToolsIn, MAGENTA));
    }
    for (pid, color) in ambient {
        let p = path(l, pid);
        let len = p.len();
        for (i, (x, y)) in p.iter().enumerate() {
            if i + 1 < len && i > 0 && (i as u64 + 600 - tick % 600) % 5 == 0 {
                cv.put(*x, *y, "•", st(color), w);
            }
        }
    }
}

fn session_box(cv: &mut Cv, l: &Layout, s: &SessionState, ms: &MapState, now: i64, tick: u64) {
    let flash = now - ms.session_flash < FLASH_MS;
    let color = if flash { WHITE } else { YELLOW };
    let title = if s.info.busy {
        format!("{} {}", t("SESSION PRINCIPALE", "MAIN SESSION"), SPIN[(tick % 4) as usize])
    } else {
        t("SESSION PRINCIPALE", "MAIN SESSION").to_string()
    };
    frame_box(cv, l.sess, false, color, &title, YELLOW);
    let model = s.model.clone().unwrap_or_else(|| "—".into());
    line(cv, l.sess, 0, &[seg(model, WHITE), seg(" · lead · ", DGRAY), seg(s.info.project.clone(), CYAN)]);
    let mut eff = vec![seg("effort ", DGRAY)];
    eff.extend(effort_meter(s.effort.as_deref()));
    line(cv, l.sess, 1, &eff);
    line(
        cv,
        l.sess,
        2,
        &[seg(format!("ctx {} · {} {} · {} {}", fmt_k(s.ctx_tokens), s.prompts, crate::i18n::turns(s.prompts), s.tools, t("outils", "tools")), LGRAY)],
    );
    if s.info.busy {
        line(cv, l.sess, 3, &[seg("● BUSY", LGREEN), seg(format!(" · {}", fmt_elapsed(now - s.info.started_at_ms)), LGRAY)]);
    } else {
        let since = fmt_elapsed(now - s.info.updated_at_ms.max(s.last_activity_ms));
        line(cv, l.sess, 3, &[seg("○ IDLE", DGRAY), seg(format!(" · {} {since}", t("inactif", "idle")), DGRAY)]);
    }
}

fn human_box(cv: &mut Cv, l: &Layout, s: &SessionState, now: i64) {
    let r = l.human;
    frame_box(cv, r, true, LMAGENTA, t("HUMAIN", "HUMAN"), LMAGENTA);
    let iw = inner_w(r);
    let age = if s.last_prompt_ms == 0 { String::new() } else if fr() { format!(" · il y a {}", fmt_elapsed(now - s.last_prompt_ms)) } else { format!(" · {} ago", fmt_elapsed(now - s.last_prompt_ms)) };
    line(cv, r, 0, &[seg("prompt", DGRAY), seg(age, DGRAY)]);
    let text = if s.last_prompt.is_empty() { "—".to_string() } else if fr() { format!("« {} »", s.last_prompt) } else { format!("“{}”", s.last_prompt) };
    let rows = r.height.saturating_sub(4) as usize;
    for (i, t) in wrap(&text, iw, rows).iter().enumerate() {
        line(cv, r, 1 + i as u16, &[seg(t.clone(), YELLOW)]);
    }
    line(cv, r, r.height - 3, &[seg(format!("{} ", crate::i18n::turns(s.prompts)), DGRAY), seg(s.prompts.to_string(), WHITE)]);
}

fn models_box(cv: &mut Cv, l: &Layout, s: &SessionState) {
    let r = l.models;
    frame_box(cv, r, true, LBLUE, t("MODÈLES", "MODELS"), LBLUE);
    let model = s.model.clone().unwrap_or_else(|| "—".into());
    let advisor = s.advisor.as_deref().map(short_model).unwrap_or_else(|| "—".into());
    line(cv, r, 0, &[seg(fit(t("principal", "main"), 11), DGRAY), seg(model, WHITE)]);
    line(cv, r, 1, &[seg(fit(t("conseiller", "advisor"), 11), DGRAY), seg(advisor, LBLUE)]);
    let mut eff = vec![seg(fit("effort", 11), DGRAY)];
    eff.extend(effort_meter(s.effort.as_deref()));
    line(cv, r, 2, &eff);
    line(cv, r, 3, &[seg(fit(t("contexte", "context"), 11), DGRAY), seg(format!("{} tok", fmt_k(s.ctx_tokens)), LGRAY)]);
    line(cv, r, 4, &[seg(fit("claude", 11), DGRAY), seg(format!("v{}", s.info.version), DGRAY)]);
}

fn tools_box(cv: &mut Cv, l: &Layout, s: &SessionState) {
    let r = l.tools;
    frame_box(cv, r, true, LGREEN, t("OUTILS · récents", "TOOLS · recent"), LGREEN);
    let mut hist: Vec<(&String, &u32)> = s.tool_hist.iter().collect();
    hist.sort_by_key(|(n, c)| (Reverse(**c), (*n).clone()));
    if hist.is_empty() {
        line(cv, r, 0, &[seg(t("aucun appel récent", "no recent calls"), DGRAY)]);
        return;
    }
    let max = *hist[0].1 as f64;
    let iw = inner_w(r);
    let bar_w = iw.saturating_sub(9 + 1 + 1 + 4);
    for (i, (name, count)) in hist.iter().take(r.height as usize - 2).enumerate() {
        let full = ((**count as f64 / max) * bar_w as f64).round().max(1.0) as usize;
        line(
            cv,
            r,
            i as u16,
            &[
                seg(fit(name, 9), LGRAY),
                seg(" ", LGRAY),
                seg("▆".repeat(full.min(bar_w)), LGREEN),
                seg("▁".repeat(bar_w - full.min(bar_w)), DGRAY),
                seg(format!(" {:>4}", count), WHITE),
            ],
        );
    }
}

fn last_box(cv: &mut Cv, l: &Layout, s: &SessionState) {
    let r = l.last;
    frame_box(cv, r, true, LGREEN, t("DERNIER APPEL", "LAST CALL"), LGREEN);
    let Some((name, note, ts, bytes)) = &s.last_tool else {
        line(cv, r, 0, &[seg("—", DGRAY)]);
        return;
    };
    line(cv, r, 0, &[seg(name.clone(), WHITE), seg(format!(" · {}", hhmmss(*ts)), DGRAY)]);
    let rows = r.height.saturating_sub(4) as usize;
    for (i, t) in wrap(note, inner_w(r), rows).iter().enumerate() {
        line(cv, r, 1 + i as u16, &[seg(t.clone(), LGRAY)]);
    }
    line(cv, r, r.height - 3, &[seg(fmt_bytes(*bytes), YELLOW), seg(t(" envoyés", " sent"), DGRAY)]);
}

/// Comme `fit`, mais coupe au début pour garder la fin (le nom du fichier).
fn fit_left(s: &str, w: usize) -> String {
    if width(s) <= w {
        return fit(s, w);
    }
    let chars: Vec<char> = s.chars().collect();
    let keep: String = chars[chars.len().saturating_sub(w.saturating_sub(1))..].iter().collect();
    fit(&format!("…{keep}"), w)
}

fn files_box(cv: &mut Cv, r: Rect, s: &SessionState) {
    frame_box(cv, r, true, LGREEN, t("FICHIERS · récents", "FILES · recent"), LGREEN);
    if s.files.is_empty() {
        line(cv, r, 0, &[seg(t("aucun fichier touché", "no files touched"), DGRAY)]);
        return;
    }
    let iw = inner_w(r);
    for (i, (tool, path, ts)) in s.files.iter().take(r.height as usize - 2).enumerate() {
        let (op, c) = match tool.as_str() {
            "Read" => (t("lu ", "rd "), LCYAN),
            "Write" => (t("écr", "wr "), YELLOW),
            _ => (t("mod", "ed "), YELLOW),
        };
        let pw = iw.saturating_sub(4 + 6);
        line(
            cv,
            r,
            i as u16,
            &[
                seg(op, c),
                seg(" ", LGRAY),
                seg(fit_left(path, pw), LGRAY),
                seg(format!(" {}", &hhmmss(*ts)[..5]), DGRAY),
            ],
        );
    }
}

fn swarm_box(cv: &mut Cv, r: Rect, s: &SessionState, now: i64) {
    frame_box(cv, r, true, LCYAN, &format!("{} · {}", t("ESSAIM", "SWARM"), s.agents.len()), LCYAN);
    if s.agents.is_empty() {
        line(cv, r, 0, &[seg(t("aucun agent lancé", "no agents launched"), DGRAY)]);
        return;
    }
    let mut ags: Vec<&AgentState> = s.agents.iter().collect();
    ags.sort_by_key(|a| (agent_rank(a, now), Reverse(a.last_activity_ms)));
    let iw = inner_w(r);
    let rows = r.height as usize - 2;
    let shown = if ags.len() > rows { rows - 1 } else { ags.len() };
    for (i, a) in ags.iter().take(shown).enumerate() {
        let (g, c) = match a.status(now) {
            AgentStatus::Working => ("●", LGREEN),
            AgentStatus::Waiting => ("◌", BROWN),
            AgentStatus::Done => ("✓", GREEN),
            AgentStatus::Stale => ("·", DGRAY),
        };
        let text_c = if a.status(now) == AgentStatus::Stale { DGRAY } else { LGRAY };
        line(
            cv,
            r,
            i as u16,
            &[
                seg(g, c),
                seg(" ", LGRAY),
                seg(fit(&a.agent_type, 8), LCYAN),
                seg(" ", LGRAY),
                seg(trunc(&a.desc, iw.saturating_sub(11)), text_c),
            ],
        );
    }
    if ags.len() > shown {
        line(cv, r, shown as u16, &[seg(format!("… {} {}", ags.len() - shown, t("autres", "more")), DGRAY)]);
    }
}

fn agent_box(cv: &mut Cv, r: Rect, a: Option<&AgentState>, ms: &MapState, now: i64, tick: u64) {
    let Some(a) = a else {
        frame_box(cv, r, true, DGRAY, t("libre", "free"), DGRAY);
        line(cv, r, (r.height - 2) / 2 - 1, &[seg(fit(t("en attente d'agent", "awaiting agent"), inner_w(r)), DGRAY)]);
        return;
    };
    let status = a.status(now);
    let flash = ms.agent_flash.contains_key(&a.id);
    let color = match status {
        _ if flash => WHITE,
        AgentStatus::Working => LCYAN,
        AgentStatus::Waiting => BROWN,
        AgentStatus::Done => GREEN,
        AgentStatus::Stale => DGRAY,
    };
    let iw = inner_w(r);
    frame_box(cv, r, true, color, &trunc(&a.agent_type, iw), if status == AgentStatus::Stale { DGRAY } else { LCYAN });
    let inner = r.height - 2;
    let desc_rows = inner.saturating_sub(3) as usize;
    for (i, t) in wrap(&a.desc, iw, desc_rows).iter().enumerate() {
        line(cv, r, i as u16, &[seg(t.clone(), LGRAY)]);
    }
    // barre d'activité + nombre d'outils
    let track = iw.saturating_sub(5);
    let bar: Vec<Seg> = match status {
        AgentStatus::Working => {
            let pos = (tick as usize / 2) % (track + 3);
            (0..track)
                .map(|i| if i + 3 > pos && i <= pos { seg("▓", LCYAN) } else { seg("░", DGRAY) })
                .collect()
        }
        AgentStatus::Done => vec![seg("▓".repeat(track), LGREEN)],
        AgentStatus::Waiting => vec![seg("░".repeat(track), BROWN)],
        AgentStatus::Stale => vec![seg("░".repeat(track), DGRAY)],
    };
    let mut bl = bar;
    bl.push(seg(format!(" {:>3}{}", a.tools.min(999), t("o", "c")), WHITE));
    line(cv, r, inner - 3, &bl);
    // état : version longue si elle tient, sinon courte
    let (glyph, word, t, c) = match status {
        AgentStatus::Working => (SPIN[(tick % 4) as usize], t("en cours", "working"), fmt_elapsed(now - a.started_ms), LGREEN),
        AgentStatus::Done => ("✓", t("terminé", "done"), fmt_elapsed(a.finished_ms.unwrap_or(now) - a.started_ms), LGREEN),
        AgentStatus::Waiting => ("◌", t("attente", "waiting"), fmt_elapsed(now - a.last_activity_ms), BROWN),
        AgentStatus::Stale => ("·", t("inactif", "idle"), String::new(), DGRAY),
    };
    let long = format!("{glyph} {word} {t}");
    let st_line: Vec<Seg> = if width(long.trim_end()) <= iw {
        vec![seg(format!("{glyph} {word} "), c), seg(t, LGRAY)]
    } else {
        vec![seg(format!("{glyph} "), c), seg(t, LGRAY)]
    };
    line(cv, r, inner - 2, &st_line);
    line(
        cv,
        r,
        inner - 1,
        &[
            seg(fmt_k(a.bytes_out), YELLOW),
            seg("▲ ", LCYAN),
            seg(fmt_k(a.bytes_in), YELLOW),
            seg("▼", LMAGENTA),
        ],
    );
}

fn merge_box(cv: &mut Cv, l: &Layout, s: &SessionState) {
    let r = l.merge;
    frame_box(cv, r, false, LCYAN, t("RETOUR À LA SESSION", "BACK TO SESSION"), YELLOW);
    let done: Vec<&AgentState> = s.agents.iter().filter(|a| a.finished_ms.is_some()).collect();
    let bytes: u64 = done.iter().map(|a| a.bytes_in).sum();
    line(
        cv,
        l.merge,
        0,
        &[
            seg(format!("{} {}{}", done.len(), t("résultat", "result"), if done.len() > 1 { "s" } else { "" }), WHITE),
            seg(" · ", DGRAY),
            seg(fmt_bytes(bytes), YELLOW),
            seg(t(" reçus", " received"), DGRAY),
        ],
    );
    let last = done.iter().max_by_key(|a| a.finished_ms);
    match last {
        Some(a) if !a.summary.is_empty() => line(cv, r, 1, &[seg(format!("{} : ", a.agent_type), LCYAN), seg(a.summary.clone(), LGRAY)]),
        Some(a) => line(cv, r, 1, &[seg(format!("{} : ", a.agent_type), LCYAN), seg(a.desc.clone(), LGRAY)]),
        None => line(cv, r, 1, &[seg(t("aucun résultat d'agent pour l'instant", "no agent results yet"), DGRAY)]),
    }
}

fn packets(cv: &mut Cv, l: &Layout, ms: &MapState, now: i64) {
    for p in &ms.packets {
        let t = now - p.start_ms;
        if t < 0 || t >= PACKET_MS {
            continue;
        }
        let cells = path(l, p.path);
        if cells.is_empty() {
            continue;
        }
        let idx = ((t as f64 / PACKET_MS as f64) * cells.len() as f64) as usize;
        let idx = idx.min(cells.len() - 1);
        for k in 1..=2 {
            if idx >= k {
                let (x, y) = cells[idx - k];
                if let Some(c) = cv.buf.cell_mut((x, y)) {
                    c.set_fg(p.color);
                }
            }
        }
        let (x, y) = cells[idx];
        if let Some(c) = cv.buf.cell_mut((x, y)) {
            c.set_symbol("●");
            c.set_fg(p.color);
        }
    }
}

fn journal(cv: &mut Cv, l: &Layout, app: &App, s: &SessionState) {
    let rows = (l.footer_y - l.journal_y - 1) as usize;
    let lim = l.w - 1;
    let mut entries: Vec<&FlowEntry> = app.flow.iter().rev().filter(|e| e.session_id == s.info.id).take(rows).collect();
    entries.reverse();
    if entries.is_empty() {
        cv.put(2, l.journal_y + 1, t("en attente d'événements…", "waiting for events…"), st(DGRAY), lim);
        return;
    }
    for (i, e) in entries.iter().enumerate() {
        let ev = &e.ev;
        let (src, src_c) = match &e.agent {
            Some(id) => (format!("a:{}", id.chars().take(6).collect::<String>()), LCYAN),
            None => ("lead".to_string(), YELLOW),
        };
        let (arrow, arrow_c) = match ev.dir {
            Dir::Out => ("──▶", LCYAN),
            Dir::In => ("◀──", LMAGENTA),
        };
        let kind_c = match ev.kind {
            Kind::Error => LRED,
            Kind::Prompt => YELLOW,
            Kind::Agent => LCYAN,
            Kind::Notif => LGREEN,
            Kind::System => DGRAY,
            _ => WHITE,
        };
        let size = match (ev.kind, ev.tokens) {
            (Kind::Think | Kind::Text, Some(t)) => fmt_tokens(t),
            _ => fmt_bytes(ev.bytes),
        };
        let segs: Vec<Seg> = vec![
            seg(" ", LGRAY),
            seg(hhmmss(ev.ts_ms), DGRAY),
            seg("  ", LGRAY),
            seg(fit(&src, 8), src_c),
            seg(" ", LGRAY),
            seg(arrow, arrow_c),
            seg(" ", LGRAY),
            seg(fit(ev.kind.label(), 9), kind_c),
            seg(" ", LGRAY),
            seg(fit(&ev.name, 9), LGRAY),
            seg(" ", LGRAY),
            seg(fit(&size, 8), YELLOW),
            seg(" ", LGRAY),
            seg(ev.note.clone(), if ev.kind == Kind::Error { LRED } else { LGRAY }),
        ];
        cv.segs(1, l.journal_y + 1 + i as u16, &segs, lim);
    }
}
