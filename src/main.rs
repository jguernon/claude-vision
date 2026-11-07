//! Claude Vision : écran EGA temps réel des sessions Claude Code.
mod accounts;
mod ega;
mod i18n;
mod engine;
mod map;
mod quota;
mod raster;
mod sessions;
mod state;
mod transcript;
mod ui;
mod util;
mod window;

use std::{
    io::IsTerminal,
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
};

const HELP_FR: &str = "\
claude-vision : sessions Claude Code, agents, flux et quotas en temps réel (palette EGA)

  claude-vision                ouvre sa propre fenêtre et rend la main au terminal
  claude-vision --window       ouvre la fenêtre au premier plan (utilisé par le lanceur)
  claude-vision --map          ouvre aussi la carte animée dès le lancement
  claude-vision --tui          affiche l'écran dans le terminal courant
  claude-vision --lang fr|en   choisit la langue (touche l pour basculer)
  claude-vision --snapshot F   rend une image PNG de l'écran dans F (COLS LIGNES optionnels)
  claude-vision --snapshot-map F [COLS LIGNES] [--session ID]
                               rend une image PNG de la carte d'une session

Dans la fenêtre : clic sur une session ou touche m pour ouvrir la carte animée.
";

const HELP_EN: &str = "\
claude-vision: live Claude Code sessions, agents, data flow and quotas (EGA palette)

  claude-vision                opens its own window and returns to the shell
  claude-vision --window       opens the window in the foreground (used by the launcher)
  claude-vision --map          also opens the animated map at startup
  claude-vision --tui          draws the screen in the current terminal
  claude-vision --lang fr|en   picks the language (press l to switch)
  claude-vision --snapshot F   renders a PNG of the screen to F (optional COLS ROWS)
  claude-vision --snapshot-map F [COLS ROWS] [--session ID]
                               renders a PNG of a session's map

In the window: click a session or press m to open the animated map.
";

const NO_CONFIG_FR: &str = "Aucun dossier de configuration Claude Code trouvé (~/.claude).";
const NO_CONFIG_EN: &str = "No Claude Code configuration folder found (~/.claude).";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let lang_arg = args.iter().position(|a| a == "--lang").and_then(|i| args.get(i + 1)).cloned();
    i18n::init(lang_arg.as_deref());
    if has("-h") || has("--help") {
        print!("{}", i18n::t(HELP_FR, HELP_EN));
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--snapshot" || a == "--snapshot-map") {
        let is_map = args[i] == "--snapshot-map";
        let path = args.get(i + 1).cloned().unwrap_or_else(|| "claude-vision.png".into());
        let cols = args.get(i + 2).and_then(|s| s.parse().ok()).unwrap_or(if is_map { 150 } else { 120 });
        let rows = args.get(i + 3).and_then(|s| s.parse().ok()).unwrap_or(if is_map { 84 } else { 34 });
        let Some(mut engine) = engine::Engine::start(false) else {
            eprintln!("{}", i18n::t(NO_CONFIG_FR, NO_CONFIG_EN));
            return Ok(());
        };
        engine.wait_quotas(Duration::from_secs(8));
        if let Some(j) = args.iter().position(|a| a == "--session") {
            if let Some(id) = args.get(j + 1) {
                if let Some(k) = engine.app.sessions.iter().position(|x| x.info.id.starts_with(id.as_str())) {
                    engine.app.selected = k;
                }
            }
        }
        return if is_map {
            window::snapshot_map(&engine, &path, cols, rows)
        } else {
            window::snapshot(&engine, &path, cols, rows)
        };
    }
    let has_display =
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    if has("--tui") || !has_display {
        return run_tui();
    }
    // Dans un AppImage, le programme ne doit pas se détacher : le système de fichiers de
    // l'AppImage disparaît quand le processus de départ se termine.
    let in_appimage = std::env::var_os("APPIMAGE").is_some();
    if !has("--window") && !in_appimage {
        // Lancement depuis un terminal ou un shell : la fenêtre vit seule.
        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(exe);
        cmd.arg("--window");
        if has("--map") {
            cmd.arg("--map");
        }
        if let Some(l) = &lang_arg {
            cmd.args(["--lang", l]);
        }
        cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        if std::io::stdout().is_terminal() {
            println!("{}", i18n::t("Claude Vision s'ouvre dans sa propre fenêtre.", "Claude Vision opens in its own window."));
        }
        return Ok(());
    }
    let Some(engine) = engine::Engine::start(true) else {
        eprintln!("{}", i18n::t(NO_CONFIG_FR, NO_CONFIG_EN));
        return Ok(());
    };
    window::run(engine, has("--map"))
}

fn run_tui() -> Result<()> {
    let Some(mut engine) = engine::Engine::start(true) else {
        eprintln!("{}", i18n::t(NO_CONFIG_FR, NO_CONFIG_EN));
        return Ok(());
    };
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let res = tui_loop(&mut terminal, &mut engine);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    engine.shutdown();
    res
}

fn tui_loop(terminal: &mut ratatui::DefaultTerminal, engine: &mut engine::Engine) -> Result<()> {
    loop {
        engine.update();
        let tick = engine.tick;
        let app = &engine.app;
        terminal.draw(|f| {
            let area = f.area();
            ui::draw(f.buffer_mut(), area, app, tick)
        })?;
        engine.tick += 2;
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if engine.app.on_key(k.code, k.modifiers) {
                        break;
                    }
                }
                Event::Mouse(m) => {
                    let sz = terminal.size()?;
                    let _ = engine.app.on_mouse(m, sz.width, sz.height);
                }
                _ => {}
            }
        }
    }
    Ok(())
}
