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

/// `targets` : (index du compte dans l'application, dossier de configuration).
pub fn spawn(targets: Vec<(usize, PathBuf)>, tx: Sender<QuotaResult>, refresh: Arc<AtomicBool>) {
    thread::spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(20))
            .build();
        let n = targets.len();
        let mut next = vec![0i64; n];
        let mut backoff = vec![0i64; n];
        let mut last_try = vec![0i64; n];
        let mut seen_at = vec![0i64; n];
        let mut tiers: Vec<Option<String>> = vec![None; n];
        let mut profile_at = vec![0i64; n];
        loop {
            let now = now_ms();
            let manual = refresh.swap(false, Ordering::SeqCst);
            // Une autre instance (ou un lancement précédent) a peut-être déjà relevé
            // ces quotas : on reprend ses valeurs au lieu de rappeler l'API.
            let disk = load_cache();
            for (k, (ai, dir)) in targets.iter().enumerate() {
                if let Some((at, tier, windows)) = cache_get(&disk, dir) {
                    if at > seen_at[k] && (now - at < POLL_MS || seen_at[k] == 0) {
                        seen_at[k] = at;
                        if tier.is_some() {
                            tiers[k] = tier.clone();
                            profile_at[k] = profile_at[k].max(at);
                        }
                        if now - at < POLL_MS {
                            next[k] = next[k].max(at + POLL_MS);
                        }
                        last_try[k] = last_try[k].max(at);
                        let _ = tx.send(QuotaResult {
                            account: *ai,
                            windows,
                            tier,
                            error: None,
                            fetched_at_ms: at,
                            retry_at_ms: None,
                        });
                    }
                }
            }
            for (k, (ai, dir)) in targets.iter().enumerate() {
                let due = now >= next[k] || (manual && now - last_try[k] >= MANUAL_MIN_MS);
                if !due {
                    continue;
                }
                last_try[k] = now;
                match fetch_usage(&agent, dir) {
                    Ok(windows) => {
                        backoff[k] = 0;
                        next[k] = now + POLL_MS;
                        if tiers[k].is_none() || now - profile_at[k] > PROFILE_MS {
                            if let Some(t) = fetch_tier(&agent, dir) {
                                tiers[k] = Some(t);
                            }
                            profile_at[k] = now;
                        }
                        // relire avant d'écrire : ne pas écraser les comptes d'une autre instance
                        let mut cache = load_cache();
                        cache_put(&mut cache, dir, now, tiers[k].as_deref(), &windows);
                        save_cache(&cache);
                        seen_at[k] = now;
                        let _ = tx.send(QuotaResult {
                            account: *ai,
                            windows,
                            tier: tiers[k].clone(),
                            error: None,
                            fetched_at_ms: now,
                            retry_at_ms: None,
                        });
                    }
                    Err(e) => {
                        backoff[k] = e
                            .retry_after_ms
                            .filter(|ms| *ms > 0)
                            .unwrap_or_else(|| (backoff[k] * 2).clamp(POLL_MS, BACKOFF_MAX_MS));
                        next[k] = now + backoff[k];
                        let _ = tx.send(QuotaResult {
                            account: *ai,
                            windows: vec![],
                            tier: tiers[k].clone(),
                            error: Some(e.msg),
                            fetched_at_ms: 0,
                            retry_at_ms: Some(next[k]),
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

// ── jeton OAuth ──────────────────────────────────────────────────────────
//
// Le jeton d'accès de `.credentials.json` expire après environ 8 h. Quand
// Claude Code ne tourne qu'à travers Claude Desktop, personne ne réécrit ce
// fichier : on rafraîchit alors le jeton comme le font Claude Code et
// l'extension GNOME « claude-usage », puis on réécrit le fichier (privé, 0600).

const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// Identifiant public de l'application OAuth « Claude Code ».
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// On rafraîchit un peu avant l'expiration.
const REFRESH_SKEW_MS: i64 = 5 * 60_000;
const DEFAULT_EXPIRES_MS: i64 = 8 * 3_600_000;

fn read_credentials(dir: &Path) -> Result<Value, String> {
    let txt = std::fs::read_to_string(dir.join(".credentials.json"))
        .map_err(|_| t("aucun jeton", "no token").to_string())?;
    let v: Value = serde_json::from_str(&txt).map_err(|_| t("jeton illisible", "unreadable token").to_string())?;
    if v["claudeAiOauth"]["accessToken"].as_str().is_none() {
        return Err(t("aucun jeton", "no token").to_string());
    }
    Ok(v)
}

/// Jeton encore valable (avec la marge), sinon `None`.
fn fresh_token(v: &Value) -> Option<String> {
    let o = &v["claudeAiOauth"];
    let exp = o["expiresAt"].as_i64().unwrap_or(0);
    if exp == 0 || exp - now_ms() > REFRESH_SKEW_MS {
        o["accessToken"].as_str().map(|s| s.to_string())
    } else {
        None
    }
}

/// Écrit le fichier d'identifiants de façon atomique, en mode 0600.
fn write_credentials(dir: &Path, v: &Value) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let path = dir.join(".credentials.json");
    let tmp = dir.join(".credentials.json.claude-vision.tmp");
    let text = serde_json::to_string(v).map_err(std::io::Error::other)?;
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)
}

fn refresh(agent: &ureq::Agent, dir: &Path, mut v: Value) -> Result<String, String> {
    let rt = v["claudeAiOauth"]["refreshToken"]
        .as_str()
        .ok_or_else(|| t("jeton expiré, pas de jeton de rafraîchissement", "token expired, no refresh token").to_string())?
        .to_string();
    let resp = agent
        .post(TOKEN_URL)
        .set("Accept", "application/json")
        .set("User-Agent", concat!("claude-vision/", env!("CARGO_PKG_VERSION")))
        .send_json(json!({ "grant_type": "refresh_token", "refresh_token": rt, "client_id": CLIENT_ID }));
    let data: Value = match resp {
        Ok(r) => r.into_json().map_err(|_| t("réponse illisible", "unreadable response").to_string())?,
        Err(ureq::Error::Status(400 | 401, _)) => {
            return Err(t(
                "session expirée : reconnecte Claude Code (claude, puis /login)",
                "session expired: sign in to Claude Code again (claude, then /login)",
            )
            .to_string())
        }
        Err(ureq::Error::Status(c, _)) => return Err(format!("HTTP {c}")),
        Err(_) => return Err(t("réseau indisponible", "network unavailable").to_string()),
    };
    let access = data["access_token"]
        .as_str()
        .ok_or_else(|| t("réponse illisible", "unreadable response").to_string())?
        .to_string();
    let o = &mut v["claudeAiOauth"];
    o["accessToken"] = json!(access);
    if let Some(r) = data["refresh_token"].as_str() {
        o["refreshToken"] = json!(r);
    }
    let lifetime = data["expires_in"].as_i64().map(|s| s * 1000).unwrap_or(DEFAULT_EXPIRES_MS);
    o["expiresAt"] = json!(now_ms() + lifetime);
    write_credentials(dir, &v).map_err(|_| t("écriture du jeton impossible", "cannot save the token").to_string())?;
    Ok(access)
}

/// Jeton d'accès valable : celui du fichier, ou un nouveau obtenu par rafraîchissement.
fn token(agent: &ureq::Agent, dir: &Path) -> Result<String, String> {
    let v = read_credentials(dir)?;
    if let Some(tok) = fresh_token(&v) {
        return Ok(tok);
    }
    match refresh(agent, dir, v) {
        Ok(tok) => Ok(tok),
        Err(e) => {
            // Claude Code ou l'extension GNOME a peut-être rafraîchi le fichier entre-temps.
            read_credentials(dir).ok().and_then(|v| fresh_token(&v)).ok_or(e)
        }
    }
}

fn get(agent: &ureq::Agent, url: &str, token: &str) -> Result<Value, FetchErr> {
    let err = |msg: &str| FetchErr { msg: msg.to_string(), retry_after_ms: None };
    match agent
        .get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", BETA)
        .set("Accept", "application/json")
        .set("User-Agent", concat!("claude-vision/", env!("CARGO_PKG_VERSION")))
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
    let t = token(agent, dir).map_err(|msg| FetchErr { msg, retry_after_ms: None })?;
    get(agent, USAGE_URL, &t).map(|v| parse_windows(&v))
}

fn fetch_tier(agent: &ureq::Agent, dir: &Path) -> Option<String> {
    let t = token(agent, dir).ok()?;
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
