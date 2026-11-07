//! Sonde des quotas : interroge l'endpoint OAuth d'usage pour chaque compte
//! (lecture seule, le jeton n'est jamais réécrit ni affiché).
//!
//! L'endpoint limite fortement les appels (HTTP 429) : on l'interroge toutes
//! les 5 minutes comme l'extension GNOME, on recule progressivement en cas de
//! refus, et les dernières valeurs sont gardées dans un cache sur disque pour
//! s'afficher immédiatement au lancement suivant.
use crate::i18n::{fr, t};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
        Arc,
    },
    thread,
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::util::now_ms;

pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const BETA: &str = "oauth-2025-04-20";
const POLL_MS: i64 = 300_000;
const BACKOFF_MAX_MS: i64 = 1_800_000;
const PROFILE_MS: i64 = 6 * 3_600_000;
const MANUAL_MIN_MS: i64 = 60_000;

#[derive(Clone, Debug)]
pub struct Window {
    pub label: String,
    pub pct: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct QuotaResult {
    pub account: usize,
    pub windows: Vec<Window>,
    pub tier: Option<String>,
    pub error: Option<String>,
    /// moment où `windows` a été relevé (ms depuis l'époque Unix)
    pub fetched_at_ms: i64,
    /// prochain essai prévu après une erreur
    pub retry_at_ms: Option<i64>,
}

struct FetchErr {
    msg: String,
    retry_after_ms: Option<i64>,
}

pub fn spawn(dirs: Vec<PathBuf>, tx: Sender<QuotaResult>, refresh: Arc<AtomicBool>) {
    thread::spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(20))
            .build();
        let n = dirs.len();
        let mut cache = load_cache();
        let mut next = vec![0i64; n];
        let mut backoff = vec![0i64; n];
        let mut last_try = vec![0i64; n];
        let mut tiers: Vec<Option<String>> = vec![None; n];
        let mut profile_at = vec![0i64; n];
        for (i, d) in dirs.iter().enumerate() {
            if let Some((at, tier, windows)) = cache_get(&cache, d) {
                tiers[i] = tier.clone();
                profile_at[i] = at;
                next[i] = at + POLL_MS;
                last_try[i] = at;
                let _ = tx.send(QuotaResult {
                    account: i,
                    windows,
                    tier,
                    error: None,
                    fetched_at_ms: at,
                    retry_at_ms: None,
                });
            }
        }
        loop {
            let now = now_ms();
            let manual = refresh.swap(false, Ordering::SeqCst);
            for i in 0..n {
                let due = now >= next[i] || (manual && now - last_try[i] >= MANUAL_MIN_MS);
                if !due {
                    continue;
                }
                last_try[i] = now;
                match fetch_usage(&agent, &dirs[i]) {
                    Ok(windows) => {
                        backoff[i] = 0;
                        next[i] = now + POLL_MS;
                        if tiers[i].is_none() || now - profile_at[i] > PROFILE_MS {
                            if let Some(t) = fetch_tier(&agent, &dirs[i]) {
                                tiers[i] = Some(t);
                            }
                            profile_at[i] = now;
                        }
                        cache_put(&mut cache, &dirs[i], now, tiers[i].as_deref(), &windows);
                        save_cache(&cache);
                        let _ = tx.send(QuotaResult {
                            account: i,
                            windows,
                            tier: tiers[i].clone(),
                            error: None,
                            fetched_at_ms: now,
                            retry_at_ms: None,
                        });
                    }
                    Err(e) => {
                        backoff[i] = e
                            .retry_after_ms
                            .filter(|ms| *ms > 0)
                            .unwrap_or_else(|| (backoff[i] * 2).clamp(POLL_MS, BACKOFF_MAX_MS));
                        next[i] = now + backoff[i];
                        let _ = tx.send(QuotaResult {
                            account: i,
                            windows: vec![],
                            tier: tiers[i].clone(),
                            error: Some(e.msg),
                            fetched_at_ms: 0,
                            retry_at_ms: Some(next[i]),
                        });
                    }
                }
            }
            thread::sleep(Duration::from_millis(500));
        }
    });
}

// ── cache sur disque : ~/.cache/claude-vision/quotas.json ─────────────────

fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("claude-vision").join("quotas.json"))
}

fn load_cache() -> Value {
    cache_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &Value| v.is_object())
        .unwrap_or_else(|| json!({}))
}

fn save_cache(cache: &Value) {
    let Some(p) = cache_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, cache.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

fn cache_key(dir: &Path) -> String {
    dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()).display().to_string()
}

fn cache_get(cache: &Value, dir: &Path) -> Option<(i64, Option<String>, Vec<Window>)> {
    let e = &cache[cache_key(dir)];
    let at = e["at"].as_i64()?;
    let windows = e["windows"]
        .as_array()?
        .iter()
        .filter_map(|w| {
            Some(Window {
                label: normalize_key(w["label"].as_str()?),
                pct: w["pct"].as_f64()?,
                resets_at: parse_dt(&w["resets_at"]),
            })
        })
        .collect();
    Some((at, e["tier"].as_str().map(|s| s.to_string()), windows))
}

fn cache_put(cache: &mut Value, dir: &Path, at: i64, tier: Option<&str>, windows: &[Window]) {
    let ws: Vec<Value> = windows
        .iter()
        .map(|w| json!({"label": w.label, "pct": w.pct, "resets_at": w.resets_at.map(|d| d.to_rfc3339())}))
        .collect();
    if let Some(o) = cache.as_object_mut() {
        o.insert(cache_key(dir), json!({"at": at, "tier": tier, "windows": ws}));
    }
}

fn token(dir: &Path) -> Result<String, String> {
    let txt = std::fs::read_to_string(dir.join(".credentials.json"))
        .map_err(|_| t("aucun jeton", "no token").to_string())?;
    let v: Value = serde_json::from_str(&txt).map_err(|_| t("jeton illisible", "unreadable token").to_string())?;
    let o = &v["claudeAiOauth"];
    let tok = o["accessToken"]
        .as_str()
        .ok_or_else(|| t("aucun jeton", "no token").to_string())?
        .to_string();
    if let Some(exp) = o["expiresAt"].as_i64() {
        if exp < Utc::now().timestamp_millis() {
            return Err(t("jeton expiré (relance claude)", "token expired (restart claude)").into());
        }
    }
    Ok(tok)
}

fn get(agent: &ureq::Agent, url: &str, token: &str) -> Result<Value, FetchErr> {
    let err = |msg: &str| FetchErr { msg: msg.to_string(), retry_after_ms: None };
    match agent
        .get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", BETA)
        .set("Accept", "application/json")
        .set("User-Agent", "claude-vision/0.1")
        .call()
    {
        Ok(r) => r.into_json::<Value>().map_err(|_| err(t("réponse illisible", "unreadable response"))),
        Err(ureq::Error::Status(429, r)) => Err(FetchErr {
            msg: t("API saturée (429)", "API rate-limited (429)").into(),
            retry_after_ms: r
                .header("retry-after")
                .and_then(|v| v.trim().parse::<i64>().ok())
                .map(|s| s * 1000),
        }),
        Err(ureq::Error::Status(c, _)) => Err(err(&format!("HTTP {c}"))),
        Err(_) => Err(err(t("réseau indisponible", "network unavailable"))),
    }
}

fn fetch_usage(agent: &ureq::Agent, dir: &Path) -> Result<Vec<Window>, FetchErr> {
    let t = token(dir).map_err(|msg| FetchErr { msg, retry_after_ms: None })?;
    get(agent, USAGE_URL, &t).map(|v| parse_windows(&v))
}

fn fetch_tier(agent: &ureq::Agent, dir: &Path) -> Option<String> {
    let t = token(dir).ok()?;
    let v = get(agent, PROFILE_URL, &t).ok()?;
    let org = &v["organization"];
    Some(crate::accounts::tier_label(
        org["plan_display_label"].as_str(),
        org["organization_type"].as_str(),
        None,
        org["rate_limit_tier"].as_str(),
    ))
}

fn parse_dt(v: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn capitalize(s: &str) -> String {
    let mut out = s.to_string();
    if let Some(c) = out.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    out
}

fn parse_windows(v: &Value) -> Vec<Window> {
    let mut out = Vec::new();
    if let Some(limits) = v["limits"].as_array() {
        for l in limits {
            let Some(pct) = l["percent"].as_f64() else {
                continue;
            };
            let label = match l["kind"].as_str().unwrap_or("") {
                "session" => "5h".to_string(),
                "weekly_all" => "7d".to_string(),
                "weekly_scoped" => format!(
                    "7d {}",
                    l["scope"]["model"]["display_name"]
                        .as_str()
                        .unwrap_or("model")
                ),
                other => other.replace('_', " "),
            };
            out.push(Window {
                label,
                pct,
                resets_at: parse_dt(&l["resets_at"]),
            });
        }
    }
    if out.is_empty() {
        // anciennes clés plates
        let mut push = |label: String, w: &Value| {
            if let Some(pct) = w["utilization"].as_f64() {
                out.push(Window {
                    label,
                    pct,
                    resets_at: parse_dt(&w["resets_at"]),
                });
            }
        };
        if v["five_hour"].is_object() {
            push("5h".into(), &v["five_hour"]);
        }
        if v["seven_day"].is_object() {
            push("7d".into(), &v["seven_day"]);
        }
        if let Some(o) = v.as_object() {
            for (k, val) in o {
                if let Some(m) = k.strip_prefix("seven_day_") {
                    if val.is_object() && !matches!(m, "oauth_apps" | "cowork") {
                        push(format!("7d {}", capitalize(m)), val);
                    }
                }
            }
        }
    }
    out.sort_by_key(|w| match w.label.as_str() {
        "5h" => 0,
        "7d" => 1,
        _ => 2,
    });
    out
}

/// Clé interne d'une fenêtre (« 7j Fable » des anciens caches → « 7d Fable »).
fn normalize_key(k: &str) -> String {
    match k.strip_prefix("7j") {
        Some(rest) => format!("7d{rest}"),
        None => k.to_string(),
    }
}

/// Libellé affiché d'une fenêtre : « 7d » devient « 7j » en français.
pub fn window_label(key: &str) -> String {
    match key.strip_prefix("7d") {
        Some(rest) if fr() => format!("7j{rest}"),
        _ => key.to_string(),
    }
}

pub fn fmt_reset(d: Option<DateTime<Utc>>) -> String {
    let Some(d) = d else {
        return "—".into();
    };
    let secs = (d - Utc::now()).num_seconds();
    if secs <= 0 {
        return t("bientôt", "soon").into();
    }
    let (dd, hh, mm) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60);
    if dd > 0 {
        format!("{dd}{} {hh}h", t("j", "d"))
    } else if hh > 0 {
        format!("{hh}h {mm:02}")
    } else {
        format!("{mm} min")
    }
}
