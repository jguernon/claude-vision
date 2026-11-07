//! Registre des sessions Claude Code vivantes : `<config>/sessions/<pid>.json`.
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::accounts::Account;

#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub pid: u32,
    pub id: String,
    /// 8 premiers caractères de l'identifiant
    pub short: String,
    pub cwd: String,
    pub project: String,
    pub name: String,
    pub busy: bool,
    pub kind: String,
    pub version: String,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
    pub account: usize,
}

pub fn scan(accounts: &[Account]) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    for (ai, acc) in accounts.iter().enumerate() {
        let Ok(rd) = fs::read_dir(acc.dir.join("sessions")) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Ok(txt) = fs::read_to_string(&p) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(&txt) else {
                continue;
            };
            let Some(pid) = v["pid"].as_u64() else {
                continue;
            };
            if !alive(pid as u32, v["procStart"].as_str()) {
                continue;
            }
            let Some(id) = v["sessionId"].as_str() else {
                continue;
            };
            let cwd = v["cwd"].as_str().unwrap_or("").to_string();
            let project = Path::new(&cwd)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&cwd)
                .to_string();
            let name = v["name"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| project.clone());
            out.push(SessionInfo {
                pid: pid as u32,
                id: id.to_string(),
                short: id.chars().take(8).collect(),
                cwd,
                project,
                name,
                busy: v["status"].as_str() == Some("busy"),
                kind: v["kind"].as_str().unwrap_or("").to_string(),
                version: v["version"].as_str().unwrap_or("").to_string(),
                started_at_ms: v["startedAt"].as_i64().unwrap_or(0),
                updated_at_ms: v["statusUpdatedAt"]
                    .as_i64()
                    .or(v["updatedAt"].as_i64())
                    .unwrap_or(0),
                account: ai,
            });
        }
    }
    out
}

/// Le processus existe et son instant de démarrage (champ 22 de /proc/pid/stat)
/// correspond à `procStart`, sinon le fichier est un reliquat.
fn alive(pid: u32, proc_start: Option<&str>) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(ps) = proc_start else {
        return true;
    };
    let after = stat.rsplit(')').next().unwrap_or("");
    after.split_whitespace().nth(19) == Some(ps)
}

/// Chemin du transcript JSONL d'une session : `<config>/projects/<slug>/<id>.jsonl`.
pub fn transcript_path(account_dir: &Path, cwd: &str, id: &str) -> Option<PathBuf> {
    let slug: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let projects = account_dir.join("projects");
    let p = projects.join(&slug).join(format!("{id}.jsonl"));
    if p.is_file() {
        return Some(p);
    }
    // repli : chercher dans tous les projets du compte
    for e in fs::read_dir(&projects).ok()?.flatten() {
        let p = e.path().join(format!("{id}.jsonl"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}
