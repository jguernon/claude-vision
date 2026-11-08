//! État de l'application : sessions vivantes, agents, flux d'événements, quotas.
use crate::i18n::t;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{atomic::{AtomicBool, Ordering}, Arc},
};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::Value;

use crate::{
    accounts::Account,
    quota::QuotaResult,
    sessions::{self, SessionInfo},
    transcript::{parse_line, quick_counts, Item, Kind, RawEvent, Tailer},
    util::{now_ms, short_model},
};

const FLOW_CAP: usize = 3000;
const AGENT_WORKING_MS: i64 = 60_000;
const AGENT_WAITING_MS: i64 = 300_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentStatus {
    Working,
    Waiting,
    Done,
    Stale,
}

pub struct AgentState {
    pub id: String,
    pub agent_type: String,
    pub desc: String,
    pub tool_use_id: String,
    pub tailer: Tailer,
    pub names: HashMap<String, String>,
    pub tools: u32,
    pub bytes_out: u64,
    pub bytes_in: u64,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub last_activity_ms: i64,
    pub summary: String,
}

impl AgentState {
    pub fn status(&self, now: i64) -> AgentStatus {
        if self.finished_ms.is_some() {
            return AgentStatus::Done;
        }
        let idle = now - self.last_activity_ms;
        if idle < AGENT_WORKING_MS {
            AgentStatus::Working
        } else if idle < AGENT_WAITING_MS {
            AgentStatus::Waiting
        } else {
            AgentStatus::Stale
        }
    }
}

struct PendingLaunch {
    bytes: u64,
    ts_ms: i64,
}

pub struct SessionState {
    pub info: SessionInfo,
    pub effort: Option<String>,
    pub advisor: Option<String>,
    /// appels d'outils de la session principale, par nom (activité récente)
    pub tool_hist: HashMap<String, u32>,
    /// dernier appel d'outil : (nom, note, horodatage, octets)
    pub last_tool: Option<(String, String, i64, u64)>,
    pub last_prompt_ms: i64,
    /// fichiers récemment lus ou modifiés : (outil, chemin court, horodatage), le plus récent en tête
    pub files: VecDeque<(String, String, i64)>,
    pub tailer: Option<Tailer>,
    pub names: HashMap<String, String>,
    pub model: Option<String>,
    pub ctx_tokens: u64,
    pub prompts: u32,
    pub tools: u32,
    pub agents: Vec<AgentState>,
    pub last_prompt: String,
    pub last_activity_ms: i64,
    pub transcript_path: Option<PathBuf>,
    pub agents_dir: Option<PathBuf>,
    pending: HashMap<String, PendingLaunch>,
}

#[derive(Clone, Debug)]
pub struct FlowEntry {
    /// numéro d'ordre global, croissant
    pub seq: u64,
    pub ev: RawEvent,
    pub session_short: String,
    pub session_id: String,
    pub agent: Option<String>,
    pub account: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    All,
    Tools,
    Agents,
    Model,
}

impl Filter {
    pub fn next(self) -> Self {
        match self {
            Filter::All => Filter::Tools,
            Filter::Tools => Filter::Agents,
            Filter::Agents => Filter::Model,
            Filter::Model => Filter::All,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => t("tous", "all"),
            Filter::Tools => t("outils", "tools"),
            Filter::Agents => "agents",
            Filter::Model => t("modèle", "model"),
        }
    }
    pub fn accepts(self, e: &FlowEntry) -> bool {
        match self {
            Filter::All => true,
            Filter::Tools => matches!(e.ev.kind, Kind::Tool | Kind::Result | Kind::Error),
            Filter::Agents => e.agent.is_some() || matches!(e.ev.kind, Kind::Agent | Kind::Notif),
            Filter::Model => matches!(e.ev.kind, Kind::Think | Kind::Text | Kind::Prompt),
        }
    }
}

pub struct App {
    pub flow_seq: u64,
    /// compte affiché (None : tous)
    pub account_filter: Option<usize>,
    /// vrai si --account impose le compte (pas de bascule)
    pub account_locked: bool,
    pub accounts: Vec<Account>,
    pub sessions: Vec<SessionState>,
    pub flow: VecDeque<FlowEntry>,
    pub quotas: Vec<Option<QuotaResult>>,
    pub selected: usize,
    pub paused: bool,
    pub pause_len: usize,
    pub filter: Filter,
    pub only_selected: bool,
    pub agents_all: bool,
    pub show_detail: bool,
    pub refresh: Arc<AtomicBool>,
    watcher: Option<RecommendedWatcher>,
    watched: HashSet<PathBuf>,
}

impl App {
    pub fn new(accounts: Vec<Account>, watcher: Option<RecommendedWatcher>, refresh: Arc<AtomicBool>) -> Self {
        let n = accounts.len();
        let mut app = App {
            flow_seq: 0,
            account_filter: None,
            account_locked: false,
            accounts,
            sessions: Vec::new(),
            flow: VecDeque::new(),
            quotas: vec![None; n],
            selected: 0,
            paused: false,
            pause_len: 0,
            filter: Filter::All,
            only_selected: false,
            agents_all: true,
            show_detail: false,
            refresh,
            watcher,
            watched: HashSet::new(),
        };
        let dirs: Vec<PathBuf> = app.accounts.iter().map(|a| a.dir.join("sessions")).collect();
        for d in dirs {
            app.watch(&d, false);
        }
        app
    }

    fn watch(&mut self, p: &Path, recursive: bool) {
        if self.watched.contains(p) || !p.exists() {
            return;
        }
        if let Some(w) = self.watcher.as_mut() {
            let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
            if w.watch(p, mode).is_ok() {
                self.watched.insert(p.to_path_buf());
            }
        }
    }

    pub fn busy_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.info.busy).count()
    }

    pub fn set_quota(&mut self, q: QuotaResult) {
        if let Some(slot) = self.quotas.get_mut(q.account) {
            let mut q = q;
            if let Some(old) = slot.as_ref() {
                if q.tier.is_none() {
                    q.tier = old.tier.clone();
                }
                // en cas d'erreur, on garde les dernières valeurs connues
                if q.error.is_some() && q.windows.is_empty() && !old.windows.is_empty() {
                    q.windows = old.windows.clone();
                    q.fetched_at_ms = old.fetched_at_ms;
                }
            }
            *slot = Some(q);
        }
    }

    /// Lignes du panneau QUOTAS : une par fenêtre de quota, plus une ligne vide entre deux comptes.
    pub fn quota_rows(&self) -> u16 {
        let shown = self.visible_accounts();
        let lines = shown
            .iter()
            .map(|&i| self.quotas.get(i).and_then(|q| q.as_ref()).map(|q| q.windows.len().max(1)).unwrap_or(1) as u16)
            .sum::<u16>();
        (lines + shown.len().saturating_sub(1) as u16).max(1)
    }

    /// Dernières entrées du flux visibles selon le filtre, la pause et la sélection.
    pub fn visible_flow(&self, rows: usize) -> Vec<&FlowEntry> {
        let end = if self.paused { self.pause_len.min(self.flow.len()) } else { self.flow.len() };
        let sel_id = self.sessions.get(self.selected).map(|s| s.info.id.as_str());
        let mut out: Vec<&FlowEntry> = self
            .flow
            .iter()
            .take(end)
            .rev()
            .filter(|e| self.filter.accepts(e))
            .filter(|e| !self.only_selected || Some(e.session_id.as_str()) == sel_id)
            .take(rows)
            .collect();
        out.reverse();
        out
    }

    pub fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) -> bool {
        if mods.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c') | KeyCode::Char('d')) {
            return true;
        }
        match code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc => {
                if self.show_detail {
                    self.show_detail = false;
                } else {
                    return true;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.sessions.len() {
                    self.selected += 1;
                }
            }
            KeyCode::Enter | KeyCode::Char('d') => self.show_detail = !self.show_detail,
            KeyCode::Char('a') => self.agents_all = !self.agents_all,
            KeyCode::Char('f') => self.filter = self.filter.next(),
            KeyCode::Char('l') => crate::i18n::toggle(),
            KeyCode::Tab => self.cycle_account(),
            KeyCode::Char('s') => self.only_selected = !self.only_selected,
            KeyCode::Char('p') => {
                self.paused = !self.paused;
                self.pause_len = self.flow.len();
            }
            KeyCode::Char('r') => {
                self.refresh.store(true, Ordering::SeqCst);
                self.rescan_sessions();
                self.poll();
            }
            _ => {}
        }
        false
    }

    /// Clic gauche : sélectionne la session sous le curseur ; molette : change de session.
    pub fn on_mouse(&mut self, m: MouseEvent, w: u16, h: u16) -> bool {
        let Some(g) = crate::ui::geometry(self, w, h) else {
            return false;
        };
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.show_detail {
                    self.show_detail = false;
                    return false;
                }
                let in_sessions = m.column >= 1
                    && m.column <= g.l_w
                    && m.row > g.top_y
                    && m.row < g.mid_y;
                if in_sessions {
                    let idx = g.first_session + ((m.row - g.top_y - 1) / 2) as usize;
                    if idx < self.sessions.len() {
                        self.selected = idx;
                        return true;
                    }
                }
            }
            MouseEventKind::ScrollUp => self.selected = self.selected.saturating_sub(1),
            MouseEventKind::ScrollDown => {
                if self.selected + 1 < self.sessions.len() {
                    self.selected += 1;
                }
            }
            _ => {}
        }
        false
    }

    // ── sessions ──────────────────────────────────────────────────────────

    /// Comptes affichés (tous, ou seulement le compte actif).
    pub fn visible_accounts(&self) -> Vec<usize> {
        match self.account_filter {
            Some(i) if i < self.accounts.len() => vec![i],
            _ => (0..self.accounts.len()).collect(),
        }
    }

    /// Tab : compte suivant, puis « tous », puis le premier.
    pub fn cycle_account(&mut self) {
        let n = self.accounts.len();
        if self.account_locked || n < 2 {
            return;
        }
        self.account_filter = match self.account_filter {
            Some(i) if i + 1 < n => Some(i + 1),
            Some(_) => None,
            None => Some(0),
        };
        self.sessions.clear();
        self.flow.clear();
        self.selected = 0;
        self.pause_len = 0;
        self.show_detail = false;
        self.rescan_sessions();
    }

    pub fn rescan_sessions(&mut self) {
        let filter = self.account_filter;
        let found: Vec<SessionInfo> = sessions::scan(&self.accounts)
            .into_iter()
            .filter(|f| filter.map_or(true, |a| f.account == a))
            .collect();
        let now = now_ms();
        // sessions terminées
        let mut i = 0;
        while i < self.sessions.len() {
            if found.iter().any(|f| f.id == self.sessions[i].info.id) {
                i += 1;
            } else {
                let s = self.sessions.remove(i);
                self.push_flow(
                    RawEvent {
                        ts_ms: now,
                        dir: crate::transcript::Dir::In,
                        kind: Kind::System,
                        name: "session".into(),
                        bytes: 0,
                        tokens: None,
                        link: None,
                        note: format!("{} {}", t("session terminée :", "session ended:"), s.info.name),
                    },
                    &s.info,
                    None,
                );
            }
        }
        for f in found {
            if let Some(s) = self.sessions.iter_mut().find(|s| s.info.id == f.id) {
                s.info.busy = f.busy;
                s.info.name = f.name;
                s.info.updated_at_ms = f.updated_at_ms;
            } else {
                self.add_session(f);
            }
        }
        self.sessions.sort_by_key(|s| (s.info.account, s.info.started_at_ms));
        if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len().saturating_sub(1);
        }
    }

    fn add_session(&mut self, info: SessionInfo) {
        let path = sessions::transcript_path(&self.accounts[info.account].dir, &info.cwd, &info.id);
        let st = SessionState {
            info,
            effort: None,
            advisor: None,
            tool_hist: HashMap::new(),
            last_tool: None,
            last_prompt_ms: 0,
            files: VecDeque::new(),
            tailer: None,
            names: HashMap::new(),
            model: None,
            ctx_tokens: 0,
            prompts: 0,
            tools: 0,
            agents: Vec::new(),
            last_prompt: String::new(),
            last_activity_ms: 0,
            transcript_path: path.clone(),
            agents_dir: path.as_ref().map(|p| p.with_extension("").join("subagents")),
            pending: HashMap::new(),
        };
        let idx = self.sessions.len();
        self.sessions.push(st);
        if let Some(p) = path {
            // les agents d'abord : la relecture de l'historique peut alors leur attribuer
            // leur lancement et leur fin (sinon un agent terminé resterait « en attente »)
            self.discover_agents(idx);
            let counts = quick_counts(&p);
            let mut t = Tailer::open(&p, 256 * 1024);
            let lines = t.poll();
            self.sessions[idx].tailer = Some(t);
            for l in lines {
                let items = parse_line(&l, &mut self.sessions[idx].names);
                self.apply(idx, None, items, false);
            }
            self.sessions[idx].prompts = counts.prompts;
            self.sessions[idx].tools = counts.tools;
            if let Some((text, ts)) = counts.last_prompt {
                let s = &mut self.sessions[idx];
                if ts >= s.last_prompt_ms {
                    s.last_prompt = text;
                    s.last_prompt_ms = ts;
                }
            }
            if let Some(dir) = p.parent() {
                let d = dir.to_path_buf();
                self.watch(&d, true);
            }
        }
    }

    // ── lecture incrémentale ─────────────────────────────────────────────

    pub fn poll(&mut self) {
        for si in 0..self.sessions.len() {
            let lines = self.sessions[si].tailer.as_mut().map(|t| t.poll()).unwrap_or_default();
            for l in lines {
                let items = parse_line(&l, &mut self.sessions[si].names);
                self.apply(si, None, items, true);
            }
            self.discover_agents(si);
            for ai in 0..self.sessions[si].agents.len() {
                let lines = self.sessions[si].agents[ai].tailer.poll();
                for l in lines {
                    let items = parse_line(&l, &mut self.sessions[si].agents[ai].names);
                    self.apply(si, Some(ai), items, false);
                }
            }
        }
    }

    fn discover_agents(&mut self, si: usize) {
        let Some(dir) = self.sessions[si].agents_dir.clone() else {
            return;
        };
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return;
        };
        let now = now_ms();
        for e in rd.flatten() {
            let p = e.path();
            let Some(fname) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(id) = fname.strip_prefix("agent-").and_then(|s| s.strip_suffix(".meta.json")) else {
                continue;
            };
            if self.sessions[si].agents.iter().any(|a| a.id == id) {
                continue;
            }
            let meta: Value = std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null);
            let mtime_ms = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(now);
            let jsonl = dir.join(format!("agent-{id}.jsonl"));
            let jsonl_ms = std::fs::metadata(&jsonl)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(mtime_ms);
            let activity_ms = mtime_ms.max(jsonl_ms);
            let fresh = now - activity_ms < 3_600_000;
            let tailer = Tailer::open(&jsonl, if fresh { 1 << 20 } else { 0 });
            let tool_use_id = meta["toolUseId"].as_str().unwrap_or("").to_string();
            let pending = self.sessions[si].pending.remove(&tool_use_id);
            let mut a = AgentState {
                id: id.to_string(),
                agent_type: meta["agentType"]
                    .as_str()
                    .unwrap_or("agent")
                    .trim_end_matches("-purpose")
                    .to_string(),
                desc: meta["description"].as_str().unwrap_or("").to_string(),
                tool_use_id,
                tailer,
                names: HashMap::new(),
                tools: 0,
                bytes_out: pending.as_ref().map(|p| p.bytes).unwrap_or(0),
                bytes_in: 0,
                started_ms: pending.as_ref().map(|p| p.ts_ms).unwrap_or(mtime_ms),
                finished_ms: None,
                last_activity_ms: activity_ms,
                summary: String::new(),
            };
            if !fresh {
                // ancien agent : on ne rejoue pas son historique
                let _ = a.tailer.poll();
            }
            self.sessions[si].agents.push(a);
        }
    }

    fn push_flow(&mut self, ev: RawEvent, info: &SessionInfo, agent: Option<String>) {
        self.flow_seq += 1;
        self.flow.push_back(FlowEntry {
            seq: self.flow_seq,
            ev,
            session_short: info.short.clone(),
            session_id: info.id.clone(),
            agent,
            account: info.account,
        });
        if self.flow.len() > FLOW_CAP {
            self.flow.pop_front();
            if self.paused {
                self.pause_len = self.pause_len.saturating_sub(1);
            }
        }
    }

    fn apply(&mut self, si: usize, agent_idx: Option<usize>, items: Vec<Item>, count: bool) {
        for it in items {
            match it {
                Item::Model(m) => {
                    if agent_idx.is_none() {
                        self.sessions[si].model = Some(short_model(&m));
                    }
                }
                Item::Meta { effort, advisor } => {
                    if agent_idx.is_none() {
                        let s = &mut self.sessions[si];
                        if effort.is_some() {
                            s.effort = effort;
                        }
                        if advisor.is_some() {
                            s.advisor = advisor;
                        }
                    }
                }
                Item::Ctx(c) => {
                    if agent_idx.is_none() {
                        self.sessions[si].ctx_tokens = c;
                    }
                }
                Item::Prompt(p) => {
                    if agent_idx.is_none() {
                        self.sessions[si].last_prompt = p;
                    }
                }
                Item::AgentLaunch { tool_use_id, bytes, ts_ms, .. } => {
                    if agent_idx.is_none() {
                        let s = &mut self.sessions[si];
                        if let Some(a) = s.agents.iter_mut().find(|a| a.tool_use_id == tool_use_id) {
                            a.bytes_out = a.bytes_out.max(bytes);
                            a.started_ms = ts_ms;
                        } else {
                            s.pending.insert(tool_use_id, PendingLaunch { bytes, ts_ms });
                        }
                    }
                }
                Item::AgentDone { tool_use_id, bytes, summary, ts_ms } => {
                    if agent_idx.is_none() {
                        if let Some(a) = self.sessions[si]
                            .agents
                            .iter_mut()
                            .find(|a| !a.tool_use_id.is_empty() && a.tool_use_id == tool_use_id)
                        {
                            a.finished_ms = Some(ts_ms);
                            a.bytes_in = bytes;
                            a.summary = summary;
                            a.last_activity_ms = a.last_activity_ms.max(ts_ms);
                        }
                    }
                }
                Item::Event(ev) => {
                    let s = &mut self.sessions[si];
                    s.last_activity_ms = s.last_activity_ms.max(ev.ts_ms);
                    // fichiers lus ou modifiés par la session ou ses agents
                    if ev.kind == Kind::Tool
                        && matches!(ev.name.as_str(), "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit")
                        && !ev.note.is_empty()
                    {
                        s.files.retain(|(_, p, _)| p != &ev.note);
                        s.files.push_front((ev.name.clone(), ev.note.clone(), ev.ts_ms));
                        s.files.truncate(40);
                    }
                    let mut agent_tag = None;
                    if let Some(ai) = agent_idx {
                        let a = &mut s.agents[ai];
                        a.last_activity_ms = a.last_activity_ms.max(ev.ts_ms);
                        match ev.kind {
                            Kind::Tool => a.tools += 1,
                            Kind::Prompt => {
                                if a.bytes_out == 0 {
                                    a.bytes_out = ev.bytes;
                                }
                                // le prompt initial de l'agent n'est pas un événement de flux
                                continue;
                            }
                            _ => {}
                        }
                        agent_tag = Some(a.id.clone());
                    } else {
                        match ev.kind {
                            Kind::Tool => {
                                *s.tool_hist.entry(ev.name.clone()).or_insert(0) += 1;
                                s.last_tool = Some((ev.name.clone(), ev.note.clone(), ev.ts_ms, ev.bytes));
                            }
                            Kind::Prompt => s.last_prompt_ms = ev.ts_ms,
                            _ => {}
                        }
                        if count {
                            match ev.kind {
                                Kind::Prompt => s.prompts += 1,
                                Kind::Tool => s.tools += 1,
                                _ => {}
                            }
                        }
                    }
                    let info = s.info.clone();
                    self.push_flow(ev, &info, agent_tag);
                }
            }
        }
    }
}
