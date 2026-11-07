//! Petits utilitaires de formatage.
use crate::i18n::t;
use chrono::{Local, TimeZone, Utc};
use unicode_width::UnicodeWidthChar;

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub fn hhmmss(ts_ms: i64) -> String {
    Local
        .timestamp_millis_opt(ts_ms)
        .single()
        .map(|d| d.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".into())
}

pub fn fmt_bytes(b: u64) -> String {
    if b < 1000 {
        format!("{b} {}", t("o", "B"))
    } else if b < 1_000_000 {
        format!("{:.1} {}", b as f64 / 1000.0, t("ko", "kB"))
    } else {
        format!("{:.1} {}", b as f64 / 1e6, t("Mo", "MB"))
    }
}

pub fn fmt_tokens(t: u64) -> String {
    if t < 1000 {
        format!("{t} tok")
    } else {
        format!("{:.1}k tok", t as f64 / 1000.0)
    }
}

/// 712 → "712", 1234 → "1.2k", 71 234 → "71k"
pub fn fmt_k(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else if n < 1_000_000 {
        format!("{}k", n / 1000)
    } else {
        format!("{:.1}M", n as f64 / 1e6)
    }
}

pub fn fmt_elapsed(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else {
        format!("{}h{:02}", s / 3600, (s % 3600) / 60)
    }
}

/// Écrase les blancs et coupe à `n` caractères (avec « … »).
pub fn preview(s: &str, n: usize) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = collapsed.chars().take(n).collect();
    if collapsed.chars().count() > n {
        out.push('…');
    }
    out
}

pub fn width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Complète ou tronque `s` pour occuper exactement `w` colonnes.
pub fn fit(s: &str, w: usize) -> String {
    let total = width(s);
    if total <= w {
        let mut out = String::with_capacity(w);
        out.push_str(s);
        out.extend(std::iter::repeat(' ').take(w - total));
        return out;
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += cw;
    }
    if w > 0 {
        out.push('…');
        used += 1;
    }
    out.extend(std::iter::repeat(' ').take(w.saturating_sub(used)));
    out
}

/// Tronque sans compléter.
pub fn trunc(s: &str, w: usize) -> String {
    if width(s) <= w {
        s.to_string()
    } else {
        fit(s, w).trim_end().to_string()
    }
}

/// « claude-fable-5-1 » → « Fable 5.1 », « claude-opus-4-5-20251101 » → « Opus 4.5 »
pub fn short_model(m: &str) -> String {
    let rest = m.strip_prefix("claude-").unwrap_or(m);
    let parts: Vec<&str> = rest.split('-').collect();
    let mut name: String = parts.first().copied().unwrap_or(m).to_string();
    if let Some(c) = name.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    let nums: Vec<&str> = parts
        .iter()
        .skip(1)
        .take_while(|p| p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit()))
        .copied()
        .collect();
    if nums.is_empty() {
        name
    } else {
        format!("{name} {}", nums.join("."))
    }
}
