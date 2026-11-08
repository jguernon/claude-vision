//! Profil d'instance : plusieurs copies de Claude Vision peuvent tourner côte à
//! côte, chacune avec sa propre identité d'application (GNOME peut alors les
//! placer sur des espaces de travail différents), sa couleur de bandeau et,
//! au besoin, un seul compte affiché.
//!
//!   --instance NAME   identité « claude-vision-NAME », titre « Claude Vision NAME »
//!   --account SEL     n'affiche que le compte dont le libellé ou le dossier contient SEL
//!   --accent COULEUR  couleur du bandeau : blue, red, green, cyan, magenta, brown, gray ou #RRGGBB
use std::sync::OnceLock;

use ratatui::style::Color;

use crate::ega::*;

pub struct Profile {
    pub instance: Option<String>,
    pub account: Option<String>,
    pub accent: Color,
}

static PROFILE: OnceLock<Profile> = OnceLock::new();

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_ascii_lowercase();
    Some(match s.as_str() {
        "blue" | "bleu" => BLUE,
        "red" | "rouge" => RED,
        "green" | "vert" => GREEN,
        "cyan" => CYAN,
        "magenta" => MAGENTA,
        "brown" | "brun" | "orange" => BROWN,
        "gray" | "grey" | "gris" => DGRAY,
        hex if hex.len() == 7 && hex.starts_with('#') => Color::Rgb(
            u8::from_str_radix(&hex[1..3], 16).ok()?,
            u8::from_str_radix(&hex[3..5], 16).ok()?,
            u8::from_str_radix(&hex[5..7], 16).ok()?,
        ),
        _ => return None,
    })
}

pub fn init(args: &[String]) {
    let instance = arg(args, "--instance")
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    let accent = arg(args, "--accent").as_deref().and_then(parse_color).unwrap_or(BLUE);
    let _ = PROFILE.set(Profile { instance, account: arg(args, "--account"), accent });
}

pub fn get() -> &'static Profile {
    PROFILE.get_or_init(|| Profile { instance: None, account: None, accent: BLUE })
}

/// Identité de l'application pour le gestionnaire de fenêtres (app_id Wayland, WM_CLASS X11).
pub fn app_id() -> String {
    match &get().instance {
        Some(n) => format!("claude-vision-{n}"),
        None => "claude-vision".to_string(),
    }
}

/// « Claude Vision » ou « Claude Vision WORK ».
pub fn title() -> String {
    match &get().instance {
        Some(n) => format!("Claude Vision {}", n.to_uppercase()),
        None => "Claude Vision".to_string(),
    }
}

/// Suffixe affiché dans le bandeau : « » ou « WORK ».
pub fn badge() -> Option<String> {
    get().instance.as_ref().map(|n| n.to_uppercase())
}

pub fn accent() -> Color {
    get().accent
}

/// Seule l'instance principale publie l'état pour l'indicateur du panneau GNOME.
pub fn writes_tray() -> bool {
    get().instance.is_none()
}

/// Préfixe des réglages propres à l'instance (zoom) : « » ou « work. ».
pub fn key_prefix() -> String {
    match &get().instance {
        Some(n) => format!("{n}."),
        None => String::new(),
    }
}
