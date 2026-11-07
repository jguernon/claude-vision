//! Découverte des comptes Claude Code : un dossier de configuration par compte
//! (`~/.claude`, `~/.claude-work`, …). Les libellés viennent de l'extension
//! GNOME « claude-usage » quand elle est présente, sinon du nom du dossier.
use crate::i18n::t;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Account {
    pub label: String,
    pub dir: PathBuf,
    /// Libellé du forfait (« ULTRA », « TEAM 5x »). Valeur de repli lue dans
    /// `.credentials.json` ; l'endpoint de profil l'affine ensuite.
    pub tier: String,
}

pub fn discover() -> Vec<Account> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            dirs.push(p);
        }
    }
    if let Ok(rd) = std::fs::read_dir(&home) {
        for e in rd.flatten() {
            let p = e.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with(".claude") || !p.is_dir() {
                continue;
            }
            if (p.join(".credentials.json").is_file() || p.join("sessions").is_dir())
                && !dirs.iter().any(|d| same_dir(d, &p))
            {
                dirs.push(p);
            }
        }
    }
    // ~/.claude en premier, puis ordre alphabétique
    dirs.sort_by_key(|p| {
        (
            p.file_name().map(|n| n != ".claude").unwrap_or(true),
            p.clone(),
        )
    });
    let labels = gnome_labels();
    dirs.into_iter()
        .map(|dir| {
            let label = labels
                .iter()
                .find(|(d, _)| same_dir(d, &dir))
                .map(|(_, l)| l.clone())
                .unwrap_or_else(|| default_label(&dir));
            let tier = tier_from_credentials(&dir);
            Account { label, dir, tier }
        })
        .collect()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn default_label(dir: &Path) -> String {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("claude");
    let suffix = name.trim_start_matches(".claude").trim_start_matches(['-', '_', '.']);
    if suffix.is_empty() {
        "Claude".to_string()
    } else {
        let mut s = suffix.to_string();
        if let Some(c) = s.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        format!("Claude {s}")
    }
}

/// Profils configurés dans l'extension GNOME « claude-usage » : [(configDir, label)].
fn gnome_labels() -> Vec<(PathBuf, String)> {
    let Ok(out) = Command::new("dconf")
        .args(["read", "/org/gnome/shell/extensions/claude-usage/profiles"])
        .output()
    else {
        return vec![];
    };
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // dconf renvoie une chaîne GVariant entre apostrophes (ou guillemets)
    let inner = raw
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .map(|s| s.replace("\\'", "'"))
        .or_else(|| {
            raw.strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .map(|s| s.replace("\\\"", "\""))
        })
        .unwrap_or(raw);
    let Ok(v) = serde_json::from_str::<Value>(&inner) else {
        return vec![];
    };
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    Some((
                        PathBuf::from(p["configDir"].as_str()?),
                        p["label"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn tier_from_credentials(dir: &Path) -> String {
    let Ok(txt) = std::fs::read_to_string(dir.join(".credentials.json")) else {
        return "?".into();
    };
    let Ok(v) = serde_json::from_str::<Value>(&txt) else {
        return "?".into();
    };
    let o = &v["claudeAiOauth"];
    tier_label(
        None,
        None,
        o["subscriptionType"].as_str(),
        o["rateLimitTier"].as_str(),
    )
}

/// Compose le libellé du forfait.
/// `plan_label` : « Ultra plan » (endpoint de profil) ; `org_type` : « claude_team » ;
/// `sub` : « max » (credentials) ; `rate_tier` : « default_claude_max_5x ».
pub fn tier_label(
    plan_label: Option<&str>,
    org_type: Option<&str>,
    sub: Option<&str>,
    rate_tier: Option<&str>,
) -> String {
    let mult = rate_tier
        .and_then(|t| t.rsplit('_').next())
        .filter(|s| {
            s.len() >= 2
                && s.ends_with('x')
                && s[..s.len() - 1].chars().all(|c| c.is_ascii_digit())
        })
        .unwrap_or("");
    if let Some(pl) = plan_label {
        let name = pl.trim().trim_end_matches(" plan").trim_end_matches(" Plan").to_uppercase();
        if name == "ULTRA" || mult.is_empty() {
            return name;
        }
        return format!("{name} {mult}");
    }
    let base = match org_type {
        Some("claude_team") => "TEAM".to_string(),
        Some("claude_enterprise") => t("ENTREPRISE", "ENTERPRISE").to_string(),
        Some("claude_max") => "MAX".to_string(),
        Some("claude_pro") => "PRO".to_string(),
        Some(other) => other.trim_start_matches("claude_").to_uppercase(),
        None => sub.unwrap_or("?").to_uppercase(),
    };
    // Depuis le 30 octobre 2026, Max 20x s'appelle Ultra.
    if base == "MAX" && mult == "20x" {
        return "ULTRA".into();
    }
    if mult.is_empty() {
        base
    } else {
        format!("{base} {mult}")
    }
}
