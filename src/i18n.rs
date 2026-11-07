//! Langue de l'interface : français ou anglais.
//!
//! Ordre de choix : option `--lang fr|en`, variable `CLAUDE_VISION_LANG`,
//! réglage mémorisé dans `~/.config/claude-vision/lang`, puis langue du
//! système (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`). Anglais par défaut.
//! La touche `l` bascule pendant l'utilisation et mémorise le choix.
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

static FRENCH: AtomicBool = AtomicBool::new(false);

/// Vrai si l'interface est en français.
pub fn fr() -> bool {
    FRENCH.load(Ordering::Relaxed)
}

/// Choisit le texte selon la langue courante.
pub fn t(fr_text: &'static str, en_text: &'static str) -> &'static str {
    if fr() {
        fr_text
    } else {
        en_text
    }
}

/// Code de la langue courante : « fr » ou « en ».
pub fn code() -> &'static str {
    if fr() {
        "fr"
    } else {
        "en"
    }
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("claude-vision").join("lang"))
}

fn parse(s: &str) -> Option<bool> {
    let s = s.trim().to_ascii_lowercase();
    if s.starts_with("fr") {
        Some(true)
    } else if s.starts_with("en") {
        Some(false)
    } else {
        None
    }
}

/// Détermine la langue au démarrage. `arg` : valeur de `--lang`, si fournie.
pub fn init(arg: Option<&str>) {
    let from_env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty() && v != "C" && v != "POSIX");
    let choice = arg
        .and_then(parse)
        .or_else(|| from_env("CLAUDE_VISION_LANG").as_deref().and_then(parse))
        .or_else(|| {
            config_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .as_deref()
                .and_then(parse)
        })
        .or_else(|| {
            ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
                .iter()
                .find_map(|k| from_env(k))
                .map(|v| v.to_ascii_lowercase().starts_with("fr"))
        })
        .unwrap_or(false);
    FRENCH.store(choice, Ordering::Relaxed);
}

/// Bascule français ⇄ anglais et mémorise le choix.
pub fn toggle() {
    let now = !fr();
    FRENCH.store(now, Ordering::Relaxed);
    if let Some(p) = config_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, if now { "fr\n" } else { "en\n" });
    }
}

/// « tour » / « tours » (« turn » / « turns ») selon le nombre.
pub fn turns(n: u32) -> &'static str {
    match (fr(), n <= 1) {
        (true, true) => "tour",
        (true, false) => "tours",
        (false, true) => "turn",
        (false, false) => "turns",
    }
}
