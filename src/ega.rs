//! Palette EGA 16 couleurs (valeurs exactes de l'adaptateur IBM).
use ratatui::style::Color;

pub const BLACK: Color = Color::Rgb(0x00, 0x00, 0x00);
pub const BLUE: Color = Color::Rgb(0x00, 0x00, 0xAA);
pub const GREEN: Color = Color::Rgb(0x00, 0xAA, 0x00);
pub const CYAN: Color = Color::Rgb(0x00, 0xAA, 0xAA);
pub const RED: Color = Color::Rgb(0xAA, 0x00, 0x00);
pub const MAGENTA: Color = Color::Rgb(0xAA, 0x00, 0xAA);
pub const BROWN: Color = Color::Rgb(0xAA, 0x55, 0x00);
pub const LGRAY: Color = Color::Rgb(0xAA, 0xAA, 0xAA);
pub const DGRAY: Color = Color::Rgb(0x55, 0x55, 0x55);
pub const LBLUE: Color = Color::Rgb(0x55, 0x55, 0xFF);
pub const LGREEN: Color = Color::Rgb(0x55, 0xFF, 0x55);
pub const LCYAN: Color = Color::Rgb(0x55, 0xFF, 0xFF);
pub const LRED: Color = Color::Rgb(0xFF, 0x55, 0x55);
pub const LMAGENTA: Color = Color::Rgb(0xFF, 0x55, 0xFF);
pub const YELLOW: Color = Color::Rgb(0xFF, 0xFF, 0x55);
pub const WHITE: Color = Color::Rgb(0xFF, 0xFF, 0xFF);

/// Couleur attribuée à chaque compte, dans l'ordre de découverte.
pub const ACCOUNT_COLORS: [Color; 6] = [LCYAN, LMAGENTA, LGREEN, YELLOW, LBLUE, LRED];

/// Couleur de chaque compte dans la barre du haut de GNOME (indicateur du tray) :
/// bleu pour le premier compte, rouge pour le deuxième, comme les bandeaux des deux apps.
pub const TRAY_COLORS: [Color; 6] = [LBLUE, LRED, LGREEN, YELLOW, LCYAN, LMAGENTA];

/// « #RRGGBB » d'une couleur de la palette.
pub fn hex(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02X}{g:02X}{b:02X}"),
        _ => "#AAAAAA".into(),
    }
}
