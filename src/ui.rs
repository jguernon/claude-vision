//! Rendu de l'écran : grille de caractères dessinée à la main (bordures doubles,
//! palette EGA), chaque panneau avec des colonnes fixes.
use crate::i18n::{fr, t};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};

use crate::{
    ega::*,
    quota::fmt_reset,
    state::{AgentState, AgentStatus, App, SessionState},
    transcript::{Dir, Kind},
    util::*,
};

pub(crate) struct Cv<'a> {
    pub(crate) buf: &'a mut Buffer,
    pub(crate) w: u16,
    pub(crate) h: u16,
}

pub(crate) type Seg = (String, Style);

impl<'a> Cv<'a> {
    /// Écrit `s` en (x, y) sans dépasser la colonne `lim` (exclue). Renvoie le x suivant.
    pub(crate) fn put(&mut self, x: u16, y: u16, s: &str, st: Style, lim: u16) -> u16 {
        let lim = lim.min(self.w);
        if y >= self.h || x >= lim {
            return x;
        }
        let s = trunc(s, (lim - x) as usize);
        self.buf.set_string(x, y, &s, st);
        x + width(&s) as u16
    }
    pub(crate) fn segs(&mut self, x: u16, y: u16, segs: &[Seg], lim: u16) -> u16 {
        let mut x = x;
        for (s, st) in segs {
            x = self.put(x, y, s, *st, lim);
        }
        x
    }
    pub(crate) fn fill(&mut self, x: u16, y: u16, n: u16, ch: &str, st: Style, lim: u16) -> u16 {
        let s = ch.repeat(n as usize);
        self.put(x, y, &s, st, lim)
    }
}

pub(crate) fn st(fg: Color) -> Style {
    Style::default().fg(fg).bg(BLACK)
}
pub(crate) fn stb(fg: Color, bg: Color) -> Style {
    Style::default().fg(fg).bg(bg)
}
pub(crate) fn seg(s: impl Into<String>, fg: Color) -> Seg {
    (s.into(), st(fg))
}

pub(crate) fn pct_color(p: f64) -> Color {
    if p < 50.0 {
        LGREEN
    } else if p < 80.0 {
        YELLOW
    } else {
        LRED
    }
}

pub(crate) fn bar(pct: f64, w: usize) -> Vec<Seg> {
    let full = ((pct.clamp(0.0, 100.0) / 100.0) * w as f64).round() as usize;
    vec![
        seg("▓".repeat(full), pct_color(pct)),
        seg("░".repeat(w - full), DGRAY),
    ]
}

/// Nombre de cellules (1 à 9) selon la taille, sur une échelle logarithmique.
fn size_cells(bytes: u64) -> usize {
    if bytes == 0 {
        0
    } else {
        ((bytes as f64).log2() - 7.0).ceil().clamp(1.0, 9.0) as usize
    }
}

pub struct Geometry {
    pub l_w: u16,
    pub r_w: u16,
    pub top_y: u16,
    pub top_rows: u16,
    pub mid_y: u16,
    pub flux_rows: u16,
    pub quota_border_y: u16,
    pub footer_y: u16,
    pub first_session: usize,
}

/// Découpage de l'écran ; `None` si le terminal est trop petit.
pub fn geometry(app: &App, w: u16, h: u16) -> Option<Geometry> {
    let quota_rows = app.quota_rows();
    let min_h = 1 + 1 + 4 + 1 + 4 + 1 + quota_rows + 1;
    if w < 80 || h < min_h {
        return None;
    }
    let l_w: u16 = ((w as usize - 3) * 52 / 100) as u16;
    let r_w: u16 = w - 3 - l_w;
    let footer_y = h - 1;
    let quota_border_y = footer_y - quota_rows - 1;
    let top_y: u16 = 1;
    let wanted = (app.sessions.len().max(2) as u16) * 2;
    let max_top = quota_border_y.saturating_sub(top_y + 1 + 1 + 4);
    let top_rows = wanted.min(max_top).max(4);
    let mid_y = top_y + 1 + top_rows;
    let flux_rows = quota_border_y - mid_y - 1;
    let visible = (top_rows / 2) as usize;
    let first_session = if app.selected >= visible { app.selected + 1 - visible } else { 0 };
    Some(Geometry { l_w, r_w, top_y, top_rows, mid_y, flux_rows, quota_border_y, footer_y, first_session })
}

pub fn draw(buf: &mut Buffer, area: Rect, app: &App, tick: u64) {
    let mut cv = Cv { buf, w: area.width, h: area.height };
    let (w, h) = (cv.w, cv.h);
    for y in 0..h {
        cv.fill(0, y, w, " ", st(LGRAY), w);
    }
    let Some(g) = geometry(app, w, h) else {
        let min_h = 1 + 1 + 4 + 1 + 4 + 1 + app.quota_rows() + 1;
        cv.put(1, 1, &if fr() { format!("Agrandis le terminal : minimum 80 × {min_h}") } else { format!("Enlarge the terminal: at least 80 × {min_h}") }, st(YELLOW), w);
        return;
    };
    let Geometry { l_w, r_w, top_y, top_rows, mid_y, flux_rows, quota_border_y, footer_y, first_session } = g;

    header(&mut cv, app);
    hline(&mut cv, top_y, '╔', '╗', &[(1, "SESSIONS"), (l_w + 2, "AGENTS")], Some((l_w + 1, '╦')));
    for y in top_y + 1..mid_y {
        cv.put(0, y, "║", st(LGRAY), w);
        cv.put(l_w + 1, y, "║", st(LGRAY), w);
        cv.put(w - 1, y, "║", st(LGRAY), w);
    }
    sessions_panel(&mut cv, app, top_y + 1, top_rows, 1, l_w, first_session);
    agents_panel(&mut cv, app, top_y + 1, top_rows, l_w + 2, r_w, tick);

    hline(&mut cv, mid_y, '╠', '╣', &[(1, t("FLUX TEMPS RÉEL", "LIVE FLOW"))], Some((l_w + 1, '╩')));
    for y in mid_y + 1..quota_border_y {
        cv.put(0, y, "║", st(LGRAY), w);
        cv.put(w - 1, y, "║", st(LGRAY), w);
    }
    flux_panel(&mut cv, app, mid_y + 1, flux_rows, 1, w - 2);

    hline(&mut cv, quota_border_y, '╠', '╣', &[(1, "QUOTAS")], None);
    for y in quota_border_y + 1..footer_y {
        cv.put(0, y, "║", st(LGRAY), w);
        cv.put(w - 1, y, "║", st(LGRAY), w);
    }
    quotas_panel(&mut cv, app, quota_border_y + 1, 1, w - 2);
    footer(&mut cv, app, footer_y);
    if app.show_detail {
        detail_overlay(&mut cv, app);
    }
}

fn header(cv: &mut Cv, app: &App) {
    let w = cv.w;
    let bg = BLUE;
    cv.fill(0, 0, w, " ", stb(WHITE, bg), w);
    let n_acc = app.accounts.len();
    let left: Vec<Seg> = vec![
        (" ▌".into(), stb(WHITE, bg)),
        ("CLAUDE·VISION".into(), stb(YELLOW, bg)),
        ("▐ ".into(), stb(WHITE, bg)),
        (format!("v0.1 · {n_acc} {}{}", t("compte", "account"), if n_acc > 1 { "s" } else { "" }), stb(LCYAN, bg)),
    ];
    cv.segs(0, 0, &left, w);
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let n = app.sessions.len();
    let b = app.busy_count();
    let right: Vec<Seg> = vec![
        (now, stb(WHITE, bg)),
        ("   ".into(), stb(WHITE, bg)),
        (format!("{n} session{}", if n > 1 { "s" } else { "" }), stb(LGREEN, bg)),
        (" · ".into(), stb(WHITE, bg)),
        (format!("{b} active{} ", if b > 1 && fr() { "s" } else { "" }), stb(YELLOW, bg)),
    ];
    let rw: usize = right.iter().map(|(s, _)| width(s)).sum();
    let x = w.saturating_sub(rw as u16);
    cv.segs(x, 0, &right, w);
}

pub(crate) fn hline(cv: &mut Cv, y: u16, l: char, r: char, titles: &[(u16, &str)], junction: Option<(u16, char)>) {
    let w = cv.w;
    cv.fill(0, y, w, "═", st(LGRAY), w);
    cv.put(0, y, &l.to_string(), st(LGRAY), w);
    cv.put(w - 1, y, &r.to_string(), st(LGRAY), w);
    if let Some((x, c)) = junction {
        cv.put(x, y, &c.to_string(), st(LGRAY), w);
    }
    for (x, title) in titles {
        let x2 = cv.put(*x, y, "═ ", st(LGRAY), w - 1);
        let x3 = cv.put(x2, y, title, st(YELLOW), w - 1);
        cv.put(x3, y, " ", st(LGRAY), w - 1);
    }
}

fn sessions_panel(cv: &mut Cv, app: &App, y0: u16, rows: u16, x0: u16, width: u16, first: usize) {
    let lim = x0 + width;
    if app.sessions.is_empty() {
        cv.put(x0 + 1, y0, t("aucune session Claude Code en cours", "no Claude Code session running"), st(DGRAY), lim);
        cv.put(x0 + 1, y0 + 1, t("(registre : <config>/sessions/*.json)", "(registry: <config>/sessions/*.json)"), st(DGRAY), lim);
        return;
    }
    let visible = (rows / 2) as usize;
    let proj_w = 14usize.min(width as usize / 4);
    let name_w = (width as usize).saturating_sub(9 + 1 + proj_w);
    let now = now_ms();
    for (row, (i, s)) in app.sessions.iter().enumerate().skip(first).take(visible).enumerate() {
        let y = y0 + (row as u16) * 2;
        let sel = i == app.selected;
        let bg = if sel { BLUE } else { BLACK };
        cv.fill(x0, y, width, " ", stb(LGRAY, bg), lim);
        cv.fill(x0, y + 1, width, " ", stb(LGRAY, bg), lim);
        let (puce, status, c) = if s.info.busy { ("●", "BUSY", LGREEN) } else { ("○", "IDLE", DGRAY) };
        let acc_color = ACCOUNT_COLORS[s.info.account % ACCOUNT_COLORS.len()];
        let puce_color = if app.accounts.len() > 1 { acc_color } else { c };
        let line1: Vec<Seg> = vec![
            (" ".into(), stb(LGRAY, bg)),
            (puce.into(), stb(puce_color, bg)),
            (" ".into(), stb(LGRAY, bg)),
            (status.into(), stb(c, bg)),
            ("  ".into(), stb(LGRAY, bg)),
            (fit(&s.info.name, name_w), stb(WHITE, bg)),
            (" ".into(), stb(LGRAY, bg)),
            (fit(&s.info.project, proj_w), stb(CYAN, bg)),
        ];
        cv.segs(x0, y, &line1, lim);
        let model = s.model.clone().unwrap_or_else(|| "—".into());
        let avail = (width as usize).saturating_sub(9);
        let (w_turns, w_tools, w_idle, w_t, w_o) =
            if fr() { (crate::i18n::turns(s.prompts), "outils", "inactif", "t", "o") } else { (crate::i18n::turns(s.prompts), "tools", "idle", "t", "c") };
        let (long, short) = if s.info.busy {
            let el = fmt_elapsed(now - s.info.started_at_ms);
            (
                format!("{model} · {el} · {} {w_turns} · {} {w_tools} · ctx {}", s.prompts, s.tools, fmt_k(s.ctx_tokens)),
                format!("{model} · {el} · {}{w_t} · {}{w_o} · ctx {}", s.prompts, s.tools, fmt_k(s.ctx_tokens)),
            )
        } else {
            let since = fmt_elapsed(now - s.info.updated_at_ms.max(s.last_activity_ms));
            (
                format!("{model} · {w_idle} {since} · {} {w_turns} · {} {w_tools} · ctx {}", s.prompts, s.tools, fmt_k(s.ctx_tokens)),
                format!("{model} · {w_idle} {since} · {}{w_t} · {}{w_o} · ctx {}", s.prompts, s.tools, fmt_k(s.ctx_tokens)),
            )
        };
        let det = if crate::util::width(&long) <= avail { long } else { short };
        let det_color = if s.info.busy { LGRAY } else { DGRAY };
        cv.put(x0 + 9, y + 1, &det, stb(det_color, bg), lim);
    }
}

pub(crate) fn marquee(tick: u64) -> Vec<Seg> {
    let pos = ((tick / 4) % 7) as i64; // 0..6 : balayage de 2 cellules sur 5
    let mut out = Vec::with_capacity(5);
    for i in 0..5i64 {
        let on = i == pos || i == pos - 1;
        out.push(if on { seg("▓", LGREEN) } else { seg("░", DGRAY) });
    }
    out
}

fn agent_line(a: &AgentState, branch: &str, now: i64, tick: u64, type_w: usize) -> Vec<Seg> {
    let status = a.status(now);
    let mut l: Vec<Seg> = vec![seg("  ", LGRAY), seg(branch, DGRAY), seg(" ", LGRAY), seg(fit(&a.agent_type, type_w), LCYAN), seg(" ", LGRAY)];
    match status {
        AgentStatus::Working => l.extend(marquee(tick)),
        AgentStatus::Done => l.push(seg("▓▓▓▓▓", LGREEN)),
        AgentStatus::Waiting => l.push(seg("░░░░░", BROWN)),
        AgentStatus::Stale => l.push(seg("░░░░░", DGRAY)),
    }
    l.push(seg("  ", LGRAY));
    match status {
        AgentStatus::Working => {
            l.push(seg(format!("{:>5}", fmt_k(a.bytes_out)), YELLOW));
            l.push(seg("▲ ", LCYAN));
            l.push(seg(format!("{:>3} {}", a.tools, t("outils", "tools")), LGRAY));
        }
        AgentStatus::Done => {
            let dur = a.finished_ms.unwrap_or(now) - a.started_ms;
            l.push(seg(t("terminé ✓ ", "done ✓ "), LGREEN));
            l.push(seg(format!("{:>6}", fmt_elapsed(dur)), LGRAY));
            l.push(seg(" ", LGRAY));
            l.push(seg(format!("{:>5}", fmt_k(a.bytes_in)), YELLOW));
            l.push(seg("▼", LMAGENTA));
        }
        AgentStatus::Waiting => {
            l.push(seg(t("en attente ", "waiting "), BROWN));
            l.push(seg(fmt_elapsed(now - a.last_activity_ms), LGRAY));
        }
        AgentStatus::Stale => l.push(seg(t("inactif", "idle"), DGRAY)),
    }
    l.push(seg("  ", LGRAY));
    l.push(seg(a.desc.clone(), DGRAY));
    l
}

pub(crate) fn agent_rank(a: &AgentState, now: i64) -> u8 {
    match a.status(now) {
        AgentStatus::Working => 0,
        AgentStatus::Waiting => 1,
        AgentStatus::Done => {
            if a.finished_ms.map(|f| now - f < 600_000).unwrap_or(false) {
                2
            } else {
                3
            }
        }
        AgentStatus::Stale => 3,
    }
}

fn agents_panel(cv: &mut Cv, app: &App, y0: u16, rows: u16, x0: u16, width: u16, tick: u64) {
    let lim = x0 + width;
    let now = now_ms();
    let type_w = 12usize.min(width as usize / 3);
    let shown: Vec<&SessionState> = if app.agents_all {
        app.sessions.iter().collect()
    } else {
        app.sessions.get(app.selected).into_iter().collect()
    };
    let with: Vec<&SessionState> = shown.iter().copied().filter(|s| !s.agents.is_empty()).collect();
    let none: Vec<String> = shown
        .iter()
        .filter(|s| s.agents.is_empty())
        .map(|s| s.info.short.clone())
        .collect();
    let reserve = usize::from(!none.is_empty());
    let per = if app.agents_all {
        ((rows as usize).saturating_sub(reserve) / with.len().max(1)).max(2)
    } else {
        usize::MAX
    };
    let mut lines: Vec<Vec<Seg>> = Vec::new();
    for s in with {
        lines.push(vec![
            seg(" ", LGRAY),
            seg(s.info.short.clone(), WHITE),
            seg(" ", LGRAY),
            seg(s.info.name.clone(), LGRAY),
        ]);
        let mut ags: Vec<&AgentState> = s.agents.iter().collect();
        ags.sort_by_key(|a| (agent_rank(a, now), std::cmp::Reverse(a.last_activity_ms)));
        let active: Vec<&AgentState> = ags.iter().copied().filter(|a| agent_rank(a, now) < 3).collect();
        let old = ags.len() - active.len();
        let budget = per.saturating_sub(1);
        if active.is_empty() {
            let last = ags.iter().map(|a| a.last_activity_ms).max().unwrap_or(0);
            lines.push(vec![
                seg("  └─ ", DGRAY),
                seg(if fr() { format!("{} agents · aucun actif · dernier {}", ags.len(), hhmmss(last)) } else { format!("{} agents · none active · last {}", ags.len(), hhmmss(last)) }, DGRAY),
            ]);
            continue;
        }
        let (n_show, tail) = if budget <= 1 {
            (1usize, false)
        } else if active.len() + usize::from(old > 0) > budget {
            (budget - 1, true)
        } else {
            (active.len(), old > 0)
        };
        for (k, a) in active.iter().take(n_show).enumerate() {
            let last = k + 1 == n_show && !tail;
            lines.push(agent_line(a, if last { "└─▶" } else { "├─▶" }, now, tick, type_w));
        }
        if tail {
            let hidden = active.len() - n_show;
            let mut parts: Vec<String> = Vec::new();
            if hidden > 0 {
                parts.push(if fr() { if hidden == 1 { "1 autre actif".to_string() } else { format!("{hidden} autres actifs") } } else { format!("{hidden} more active") });
            }
            if old > 0 {
                parts.push(if fr() { if old == 1 { "1 inactif".to_string() } else { format!("{old} inactifs") } } else { format!("{old} idle") });
            }
            lines.push(vec![seg("  └─ ", DGRAY), seg(format!("… {}", parts.join(" · ")), DGRAY)]);
        }
    }
    if !none.is_empty() {
        lines.push(vec![seg(" ", LGRAY), seg(if fr() { format!("{} : aucun agent", none.join(" · ")) } else { format!("{}: no agents", none.join(" · ")) }, DGRAY)]);
    }
    if lines.is_empty() {
        lines.push(vec![seg(t(" aucun agent", " no agents"), DGRAY)]);
    }
    for (i, l) in lines.iter().take(rows as usize).enumerate() {
        cv.segs(x0, y0 + i as u16, l, lim);
    }
}

fn flux_panel(cv: &mut Cv, app: &App, y0: u16, rows: u16, x0: u16, width: u16) {
    let lim = x0 + width;
    let entries = app.visible_flow(rows as usize);
    if entries.is_empty() {
        cv.put(x0 + 1, y0, t("en attente d'événements…", "waiting for events…"), st(DGRAY), lim);
        return;
    }
    for (i, e) in entries.iter().enumerate() {
        let y = y0 + i as u16;
        let ev = &e.ev;
        let (src, src_color) = match &e.agent {
            Some(id) => (format!("a:{}", id.chars().take(6).collect::<String>()), LBLUE),
            None => (e.session_short.clone(), ACCOUNT_COLORS[e.account % ACCOUNT_COLORS.len()]),
        };
        let (arrow, arrow_color) = match ev.dir {
            Dir::Out => ("──▶", LCYAN),
            Dir::In => ("◀──", LMAGENTA),
        };
        let kind_color = match ev.kind {
            Kind::Error => LRED,
            Kind::Prompt => YELLOW,
            Kind::Agent => LCYAN,
            Kind::Notif => LMAGENTA,
            Kind::System => DGRAY,
            _ => WHITE,
        };
        let size = match (ev.kind, ev.tokens) {
            (Kind::Think | Kind::Text, Some(t)) => fmt_tokens(t),
            _ => fmt_bytes(ev.bytes),
        };
        let est = match (ev.kind, ev.tokens) {
            (Kind::Think | Kind::Text, Some(t)) => t * 4,
            _ => ev.bytes,
        };
        let cells = size_cells(est);
        let (bar_ch, bar_color) = match ev.kind {
            Kind::Think => ("▒", DGRAY),
            Kind::Error => ("▓", LRED),
            _ => match ev.dir {
                Dir::Out => ("▓", GREEN),
                Dir::In => ("▓", MAGENTA),
            },
        };
        let note_color = match ev.kind {
            Kind::Notif => LGREEN,
            Kind::Error => LRED,
            _ => LGRAY,
        };
        let line: Vec<Seg> = vec![
            seg(" ", LGRAY),
            seg(hhmmss(ev.ts_ms), DGRAY),
            seg("  ", LGRAY),
            seg(fit(&src, 8), src_color),
            seg(" ", LGRAY),
            seg(arrow, arrow_color),
            seg(" ", LGRAY),
            seg(fit(ev.kind.label(), 9), kind_color),
            seg(" ", LGRAY),
            seg(fit(&ev.name, 9), LGRAY),
            seg(" ", LGRAY),
            seg(fit(&size, 8), YELLOW),
            seg(" ", LGRAY),
            seg(fit(&bar_ch.repeat(cells), 9), bar_color),
            seg(" ", LGRAY),
            seg(ev.note.clone(), note_color),
        ];
        cv.segs(x0, y, &line, lim);
    }
}

fn quotas_panel(cv: &mut Cv, app: &App, y0: u16, x0: u16, width: u16) {
    let lim = x0 + width;
    let bar_w = (width as usize).saturating_sub(58).clamp(10, 40);
    let mut y = y0;
    for (i, acc) in app.accounts.iter().enumerate() {
        let color = ACCOUNT_COLORS[i % ACCOUNT_COLORS.len()];
        let q = app.quotas.get(i).and_then(|q| q.as_ref());
        let tier = q.and_then(|q| q.tier.clone()).unwrap_or_else(|| acc.tier.clone());
        let head: Vec<Seg> = vec![
            seg(" ", LGRAY),
            seg(fit(&acc.label, 14), color),
            seg(" ", LGRAY),
            seg(fit(&tier, 9), YELLOW),
            seg(" ", LGRAY),
        ];
        let blank: Vec<Seg> = vec![seg(" ".repeat(26), LGRAY)];
        let mut rows_for_account: Vec<Vec<Seg>> = Vec::new();
        match q {
            None => rows_for_account.push(vec![seg(t("… chargement", "… loading"), DGRAY)]),
            Some(q) if q.error.is_some() && q.windows.is_empty() => {
                let retry = q.retry_at_ms.map(|ts| format!(" · {} {}", t("réessai à", "retry at"), &hhmmss(ts)[..5])).unwrap_or_default();
                rows_for_account.push(vec![seg(format!("{}{retry}", q.error.clone().unwrap_or_default()), LRED)])
            }
            Some(q) if q.windows.is_empty() => {
                rows_for_account.push(vec![seg(t("aucune fenêtre de quota renvoyée", "no quota window returned"), DGRAY)])
            }
            Some(q) => {
                for (k, w) in q.windows.iter().enumerate() {
                    let mut l: Vec<Seg> = vec![seg(fit(&crate::quota::window_label(&w.label), 9), LGRAY), seg(" ", LGRAY)];
                    l.extend(bar(w.pct, bar_w));
                    l.push(seg(format!(" {:>3}%", w.pct.round() as i64), WHITE));
                    l.push(seg("  ↻ ", DGRAY));
                    l.push(seg(fit(&fmt_reset(w.resets_at), 8), LGRAY));
                    if k == 0 {
                        if let Some(err) = &q.error {
                            let retry = q.retry_at_ms.map(|ts| format!(" · {} {}", t("réessai", "retry"), &hhmmss(ts)[..5])).unwrap_or_default();
                            l.push(seg(format!("  ⚠ {err} · {} {}{retry}", t("relevé", "as of"), &hhmmss(q.fetched_at_ms)[..5]), BROWN));
                        }
                    }
                    rows_for_account.push(l);
                }
            }
        }
        for (k, r) in rows_for_account.iter().enumerate() {
            let x = if k == 0 { cv.segs(x0, y, &head, lim) } else { cv.segs(x0, y, &blank, lim) };
            cv.segs(x, y, r, lim);
            y += 1;
        }
        // ligne vide entre deux comptes
        y += 1;
    }
}

fn footer(cv: &mut Cv, app: &App, y: u16) {
    let w = cv.w;
    cv.fill(0, y, w, "═", st(LGRAY), w);
    cv.put(0, y, "╚", st(LGRAY), w);
    cv.put(w - 1, y, "╝", st(LGRAY), w);
    let keys = [
        ("q", t("quitter", "quit")),
        ("↑↓", "session"),
        ("⏎", t("détail", "details")),
        ("m", t("carte", "map")),
        ("a", if app.agents_all { t("agents: tous", "agents: all") } else { t("agents: sélection", "agents: selected") }),
        ("f", t("filtre", "filter")),
        ("s", if app.only_selected { t("flux: sélection", "flow: selected") } else { t("flux: tout", "flow: all") }),
        ("p", "pause"),
        ("r", t("rafraîchir", "refresh")),
        ("l", t("EN", "FR")),
    ];
    let mut segs: Vec<Seg> = vec![seg("═ ", LGRAY)];
    for (i, (k, lbl)) in keys.iter().enumerate() {
        if i > 0 {
            segs.push(seg(" · ", DGRAY));
        }
        segs.push(seg(*k, YELLOW));
        segs.push(seg(" ", LGRAY));
        segs.push(seg(*lbl, LGRAY));
    }
    segs.push(seg(" ", LGRAY));
    let left_end = cv.segs(1, y, &segs, w - 1);
    // à droite : le filtre s'il n'est pas « tous », et la pause
    let mut right: Vec<Seg> = Vec::new();
    if app.filter != crate::state::Filter::All {
        right.extend([seg(t(" filtre: ", " filter: "), DGRAY), seg(app.filter.label(), LCYAN), seg(" ", LGRAY)]);
    }
    if app.paused {
        right.push((" ▌PAUSE▐ ".into(), stb(WHITE, RED)));
    }
    let rw: usize = right.iter().map(|(s, _)| width(s)).sum();
    if rw > 0 {
        let x = w.saturating_sub(rw as u16 + 1).max(left_end);
        cv.segs(x, y, &right, w - 1);
    }
}

fn detail_overlay(cv: &mut Cv, app: &App) {
    let Some(s) = app.sessions.get(app.selected) else {
        return;
    };
    let (w, h) = (cv.w, cv.h);
    let bw = (w - 4).min(96);
    let bh = (h - 2).min(24);
    let x0 = (w - bw) / 2;
    let y0 = (h - bh) / 2;
    let lim = x0 + bw;
    for y in y0..y0 + bh {
        cv.fill(x0, y, bw, " ", st(LGRAY), lim);
    }
    cv.fill(x0, y0, bw, "═", st(LCYAN), lim);
    cv.fill(x0, y0 + bh - 1, bw, "═", st(LCYAN), lim);
    for y in y0 + 1..y0 + bh - 1 {
        cv.put(x0, y, "║", st(LCYAN), lim);
        cv.put(lim - 1, y, "║", st(LCYAN), lim);
    }
    cv.put(x0, y0, "╔", st(LCYAN), lim);
    cv.put(lim - 1, y0, "╗", st(LCYAN), lim);
    cv.put(x0, y0 + bh - 1, "╚", st(LCYAN), lim);
    cv.put(lim - 1, y0 + bh - 1, "╝", st(LCYAN), lim);
    let title = format!("{} · {} ", t(" DÉTAIL", " DETAILS"), s.info.short);
    cv.put(x0 + 2, y0, &title, st(YELLOW), lim);
    cv.put(lim - 22, y0 + bh - 1, t(" ⏎ ou Esc : fermer ", " ⏎ or Esc: close "), st(DGRAY), lim);

    let now = now_ms();
    let acc = &app.accounts[s.info.account];
    let acc_color = ACCOUNT_COLORS[s.info.account % ACCOUNT_COLORS.len()];
    let label_w = 12usize;
    let inner = (bw - 4) as usize;
    let mut rows: Vec<Vec<Seg>> = vec![];
    let row = |k: &str, v: Vec<Seg>| {
        let mut r = vec![seg(fit(k, label_w), DGRAY)];
        r.extend(v);
        r
    };
    rows.push(row(t("Nom", "Name"), vec![seg(s.info.name.clone(), WHITE)]));
    rows.push(row(t("Compte", "Account"), vec![seg(acc.label.clone(), acc_color), seg(format!("  ({})", acc.dir.display()), DGRAY)]));
    rows.push(row(t("Dossier", "Folder"), vec![seg(s.info.cwd.clone(), CYAN)]));
    rows.push(row("Session", vec![seg(s.info.id.clone(), LGRAY), seg(format!("  PID {}  v{}  {}", s.info.pid, s.info.version, s.info.kind), DGRAY)]));
    rows.push(row(
        t("Modèle", "Model"),
        vec![
            seg(s.model.clone().unwrap_or_else(|| "—".into()), WHITE),
            seg(if fr() { format!("  contexte {} tok · {} tours · {} outils", fmt_k(s.ctx_tokens), s.prompts, s.tools) } else { format!("  context {} tok · {} turns · {} tools", fmt_k(s.ctx_tokens), s.prompts, s.tools) }, LGRAY),
        ],
    ));
    let state = if s.info.busy { ("BUSY", LGREEN) } else { ("IDLE", DGRAY) };
    rows.push(row(
        t("Démarrée", "Started"),
        vec![
            seg(hhmmss(s.info.started_at_ms), LGRAY),
            seg(if fr() { format!("  il y a {}  ·  dernière activité {}  ·  ", fmt_elapsed(now - s.info.started_at_ms), hhmmss(s.last_activity_ms)) } else { format!("  {} ago  ·  last activity {}  ·  ", fmt_elapsed(now - s.info.started_at_ms), hhmmss(s.last_activity_ms)) }, LGRAY),
            seg(state.0, state.1),
        ],
    ));
    if let Some(p) = &s.transcript_path {
        let size = s.tailer.as_ref().map(|t| t.size()).unwrap_or(0);
        rows.push(row("Transcript", vec![seg(format!("{}  ({})", p.display(), fmt_bytes(size)), DGRAY)]));
    }
    rows.push(vec![seg("", LGRAY)]);
    let prompt_w = inner.saturating_sub(label_w);
    let mut first = true;
    let mut chunk = String::new();
    for word in s.last_prompt.split_whitespace() {
        if width(&chunk) + width(word) + 1 > prompt_w && !chunk.is_empty() {
            rows.push(row(if first { "Prompt" } else { "" }, vec![seg(chunk.clone(), YELLOW)]));
            first = false;
            chunk.clear();
        }
        if !chunk.is_empty() {
            chunk.push(' ');
        }
        chunk.push_str(word);
        if rows.len() > 12 {
            break;
        }
    }
    if !chunk.is_empty() || first {
        rows.push(row(if first { "Prompt" } else { "" }, vec![seg(if chunk.is_empty() { "—".into() } else { chunk }, YELLOW)]));
    }
    rows.push(vec![seg("", LGRAY)]);
    rows.push(row("Agents", vec![seg(format!("{}", s.agents.len()), WHITE)]));
    let mut ags: Vec<&AgentState> = s.agents.iter().collect();
    ags.sort_by_key(|a| std::cmp::Reverse(a.started_ms));
    for a in ags.iter().take(8) {
        let (txt, c) = match a.status(now) {
            AgentStatus::Working => (t("en cours", "working").to_string(), LGREEN),
            AgentStatus::Done => (format!("{} {}", t("terminé", "done"), fmt_elapsed(a.finished_ms.unwrap_or(now) - a.started_ms)), LGRAY),
            AgentStatus::Waiting => (t("en attente", "waiting").to_string(), BROWN),
            AgentStatus::Stale => (t("inactif", "idle").to_string(), DGRAY),
        };
        rows.push(row(
            "",
            vec![
                seg(fit(&a.agent_type, 14), LCYAN),
                seg(" ", LGRAY),
                seg(fit(&txt, 16), c),
                seg(" ", LGRAY),
                seg(format!("{:>5}▲ {:>5}▼  ", fmt_k(a.bytes_out), fmt_k(a.bytes_in)), YELLOW),
                seg(a.desc.clone(), LGRAY),
            ],
        ));
    }
    for (i, r) in rows.iter().take((bh - 2) as usize).enumerate() {
        cv.segs(x0 + 2, y0 + 1 + i as u16, r, lim - 1);
    }
}
