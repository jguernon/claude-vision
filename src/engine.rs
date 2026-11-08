//! Moteur commun aux deux affichages (terminal et fenêtre) : surveille les
//! fichiers, reçoit les quotas et tient l'état de l'application à jour.
use std::{
    sync::{atomic::AtomicBool, mpsc, Arc},
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use crate::{
    accounts,
    ega::{hex, ACCOUNT_COLORS},
    quota,
    state::App,
    util::now_ms,
};

const TRAY_EVERY: Duration = Duration::from_secs(5);

/// `~/.cache/claude-vision/tray.json` : état publié pour l'indicateur du panneau GNOME.
pub fn tray_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))?;
    Some(base.join("claude-vision").join("tray.json"))
}

pub struct Engine {
    pub app: App,
    pub tick: u64,
    qrx: mpsc::Receiver<quota::QuotaResult>,
    wrx: mpsc::Receiver<notify::Result<notify::Event>>,
    last_poll: Instant,
    last_scan: Instant,
    last_tray: Option<Instant>,
    tray: bool,
}

impl Engine {
    /// `None` si aucun dossier de configuration Claude Code n'existe.
    /// `tray` : publier l'état pour l'indicateur du panneau (faux pour --snapshot).
    pub fn start(tray: bool) -> Option<Engine> {
        let accounts = accounts::discover();
        if accounts.is_empty() {
            return None;
        }
        let refresh = Arc::new(AtomicBool::new(false));
        let (qtx, qrx) = mpsc::channel();
        // --account : un seul compte affiché (et interrogé)
        let locked = crate::profile::get().account.as_deref().and_then(|sel| {
            let sel = sel.to_lowercase();
            let found = accounts.iter().position(|a| {
                a.label.to_lowercase().contains(&sel)
                    || a.dir.file_name().map(|n| n.to_string_lossy().to_lowercase().contains(&sel)).unwrap_or(false)
            });
            if found.is_none() {
                eprintln!("--account {sel} : {}", crate::i18n::t("aucun compte correspondant, tous les comptes sont affichés", "no matching account, showing all accounts"));
            }
            found
        });
        let targets = accounts
            .iter()
            .enumerate()
            .filter(|(i, _)| locked.map_or(true, |l| l == *i))
            .map(|(i, a)| (i, a.dir.clone()))
            .collect();
        quota::spawn(targets, qtx, refresh.clone());
        let (wtx, wrx) = mpsc::channel::<notify::Result<notify::Event>>();
        let watcher = notify::recommended_watcher(move |res| {
            let _ = wtx.send(res);
        })
        .ok();
        let mut app = App::new(accounts, watcher, refresh);
        // compte affiché : celui de --account, sinon le premier (Tab pour basculer)
        app.account_locked = locked.is_some();
        app.account_filter = locked.or(if app.accounts.len() > 1 { Some(0) } else { None });
        app.rescan_sessions();
        Some(Engine {
            app,
            tick: 0,
            qrx,
            wrx,
            last_poll: Instant::now(),
            last_scan: Instant::now(),
            last_tray: None,
            tray: tray && crate::profile::writes_tray(),
        })
    }

    /// Intègre les changements de fichiers et les quotas reçus depuis le dernier appel.
    pub fn update(&mut self) {
        let mut dirty = false;
        let mut sess_dirty = false;
        while let Ok(ev) = self.wrx.try_recv() {
            if let Ok(ev) = ev {
                dirty = true;
                if ev.paths.iter().any(|p| {
                    p.parent()
                        .and_then(|d| d.file_name())
                        .map(|n| n == "sessions")
                        .unwrap_or(false)
                }) {
                    sess_dirty = true;
                }
            }
        }
        while let Ok(q) = self.qrx.try_recv() {
            self.app.set_quota(q);
        }
        if sess_dirty || self.last_scan.elapsed() > Duration::from_secs(2) {
            self.app.rescan_sessions();
            self.last_scan = Instant::now();
        }
        if dirty || self.last_poll.elapsed() > Duration::from_millis(1000) {
            self.app.poll();
            self.last_poll = Instant::now();
        }
        if self.tray && self.last_tray.map(|t| t.elapsed() >= TRAY_EVERY).unwrap_or(true) {
            self.write_tray();
            self.last_tray = Some(Instant::now());
        }
    }

    fn write_tray(&self) {
        let Some(path) = tray_path() else { return };
        let app = &self.app;
        let accounts: Vec<Value> = app
            .accounts
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let q = app.quotas.get(i).and_then(|q| q.as_ref());
                let weekly = q.and_then(|q| q.windows.iter().find(|w| w.label == "7d"));
                json!({
                    "label": a.label,
                    "short": a.label.strip_prefix("Claude ").unwrap_or(&a.label),
                    "color": hex(ACCOUNT_COLORS[i % ACCOUNT_COLORS.len()]),
                    "tier": q.and_then(|q| q.tier.clone()).unwrap_or_else(|| a.tier.clone()),
                    "weekly_pct": weekly.map(|w| w.pct),
                    "weekly_resets_at": weekly.and_then(|w| w.resets_at).map(|d| d.to_rfc3339()),
                    "fetched_at_ms": q.map(|q| q.fetched_at_ms).filter(|t| *t > 0),
                    "error": q.and_then(|q| q.error.clone()),
                })
            })
            .collect();
        let v = json!({
            "version": 1,
            "lang": crate::i18n::code(),
            "pid": std::process::id(),
            "updated_ms": now_ms(),
            "sessions": app.sessions.len(),
            "busy": app.busy_count(),
            "accounts": accounts,
        });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    /// À la fermeture : retire le fichier d'état pour que l'indicateur disparaisse.
    pub fn shutdown(&self) {
        if !self.tray {
            return;
        }
        if let Some(p) = tray_path() {
            let mine = std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v["pid"].as_u64())
                == Some(std::process::id() as u64);
            if mine {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    /// Attend (au plus `max`) que les quotas de tous les comptes soient arrivés.
    pub fn wait_quotas(&mut self, max: Duration) {
        let t0 = Instant::now();
        while t0.elapsed() < max {
            self.update();
            if self.app.quotas.iter().all(|q| q.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
