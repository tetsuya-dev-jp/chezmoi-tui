use anyhow::{Context, Result, bail};
use chezmoi_tui::app::{App, PreviewOrigin};
use chezmoi_tui::domain::{ChangeKind, ListView, StatusEntry};
use chezmoi_tui::{AppConfig, ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CELL_WIDTH: usize = 9;
const CELL_HEIGHT: usize = 18;
const CANVAS_COLOR: &str = "#0e1014";
const DEFAULT_FOREGROUND: &str = "#d8dee9";

const STATUS_FIXTURE: &str = "\
diff --git a/.bashrc b/.bashrc
index 8f31a20..4c79db1 100644
--- a/.bashrc
+++ b/.bashrc
@@ -8,7 +8,10 @@
 export EDITOR=\"nvim\"
-export PAGER=\"less\"
+export VISUAL=\"nvim\"
+export PAGER=\"bat --style=plain\"
+export MANPAGER=\"sh -c 'col -bx'\"
\x20
-alias gs='git status'
+alias gs='git status --short --branch'
+alias ll='eza --icons --group-directories-first'
 alias reload='source ~/.bashrc'
+# 日本語のコメントもUTF-8で扱う
";

const GITCONFIG_PREVIEW: &str = "\
[user]
    useConfigOnly = true
[core]
    editor = nvim
    pager = delta
[init]
    defaultBranch = main
[alias]
    st = status --short --branch
    lg = log --graph --oneline --decorate
";

fn main() -> Result<()> {
    self_check();

    if env::var_os("NO_COLOR").is_some() {
        let status =
            Command::new(env::current_exe().context("failed to locate screenshot executable")?)
                .args(env::args_os().skip(1))
                .env_remove("NO_COLOR")
                .status()
                .context("failed to start screenshot child")?;
        if !status.success() {
            bail!("screenshot child failed with {status}");
        }
        return Ok(());
    }

    let output_dir = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docs/images"));
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;

    render_to_file(
        fixture_app(),
        ListView::Status,
        &output_dir.join("overview.svg"),
        120,
        34,
        false,
    )?;

    let mut managed = fixture_app();
    managed.switch_view(ListView::Managed);
    managed.selected_index = 3;
    managed.set_detail_preview(
        Path::new(".gitconfig"),
        PreviewOrigin::Destination,
        GITCONFIG_PREVIEW.to_string(),
    );
    render_to_file(
        managed,
        ListView::Managed,
        &output_dir.join("managed.svg"),
        120,
        34,
        false,
    )?;

    render_to_file(
        fixture_app(),
        ListView::Status,
        &output_dir.join("actions.svg"),
        120,
        34,
        true,
    )?;

    render_to_file(
        fixture_app(),
        ListView::Status,
        &output_dir.join("narrow.svg"),
        64,
        24,
        false,
    )?;

    println!("generated screenshots in {}", output_dir.display());
    Ok(())
}

fn fixture_app() -> App {
    let config = AppConfig {
        destination_dir: Some(PathBuf::from("/home/demo")),
        source_dir: Some(PathBuf::from("/home/demo/.local/share/chezmoi")),
        working_dir: PathBuf::from("/workspace"),
        list_ratio: 35,
        detail_ratio: 64,
        footer_help: false,
        ..AppConfig::default()
    };
    let mut app = App::new(config);

    let status = vec![
        status(".bashrc", ChangeKind::Modified, ChangeKind::Modified),
        status(
            ".config/nvim/init.lua",
            ChangeKind::None,
            ChangeKind::Modified,
        ),
        status(".config/starship.toml", ChangeKind::None, ChangeKind::Added),
        status(".gitconfig", ChangeKind::None, ChangeKind::Modified),
        status(".profile", ChangeKind::None, ChangeKind::Modified),
        status(".tmux.conf", ChangeKind::None, ChangeKind::Added),
        status("bin/sync-notes", ChangeKind::None, ChangeKind::Added),
        status(
            "scripts/dotfiles-health.sh",
            ChangeKind::None,
            ChangeKind::Run,
        ),
    ];
    let managed = [
        ".bashrc",
        ".config",
        ".config/nvim/init.lua",
        ".config/starship.toml",
        ".editorconfig",
        ".gitconfig",
        ".profile",
        ".tmux.conf",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    let unmanaged = [
        "Downloads/archive.zip",
        "Projects/notes.md",
        "Projects/site/index.html",
        "notes.txt",
        "tools/scratch.log",
        "workspace.code-workspace",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    let source = [
        "dot_bashrc",
        "dot_config",
        "dot_config/nvim/init.lua",
        "dot_config/starship.toml",
        "dot_editorconfig",
        "dot_gitconfig",
        "dot_profile",
        "dot_tmux.conf",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();

    app.apply_refresh_entries(
        status,
        managed,
        unmanaged,
        Some(PathBuf::from("/home/demo/.local/share/chezmoi")),
        source,
    );
    app.rebuild_visible_entries();
    app.logs.clear();
    app.log("refresh completed: status=8 managed=8 unmanaged=6 source=8".to_string());
    app.log("diff loaded: .bashrc".to_string());
    app.log("preview ready: .gitconfig".to_string());
    app.log("hint: press ? for keyboard help".to_string());
    app.set_detail_diff(Some(Path::new(".bashrc")), STATUS_FIXTURE.to_string());
    app
}

fn status(path: &str, actual_vs_state: ChangeKind, actual_vs_target: ChangeKind) -> StatusEntry {
    StatusEntry {
        path: PathBuf::from(path),
        actual_vs_state,
        actual_vs_target,
    }
}

fn render_to_file(
    mut app: App,
    view: ListView,
    path: &Path,
    width: u16,
    height: u16,
    actions_open: bool,
) -> Result<()> {
    if app.view != view {
        app.switch_view(view);
    }
    if actions_open {
        app.open_action_menu();
    }

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).context("failed to create test terminal")?;
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .context("failed to render UI")?;
    let (svg, terminal_text) = render_svg(terminal.backend().buffer());
    assert_export_privacy(&terminal_text, "rendered terminal text")?;
    assert_export_privacy(&svg, "rendered SVG")?;
    fs::write(path, svg).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn render_svg(buffer: &Buffer) -> (String, String) {
    let width = buffer.area.width as usize * CELL_WIDTH;
    let height = buffer.area.height as usize * CELL_HEIGHT;
    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\" role=\"img\" aria-label=\"chezmoi-tui screenshot\">\
         <style>text{{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,'DejaVu Sans Mono',monospace;\
         font-size:14px;\
         white-space:pre}}.blink{{animation:blink 1s steps(1,end) infinite}}\
         .rapid-blink{{animation:blink .5s steps(1,end) infinite}}\
         @keyframes blink{{50%{{opacity:0}}}}</style>\
         <rect width=\"{width}\" height=\"{height}\" fill=\"{CANVAS_COLOR}\"/>"
    );

    for y in 0..buffer.area.height {
        let mut x = 0;
        while x < buffer.area.width {
            let background = cell_background(&buffer[(x, y)]);
            let start = x;
            x += 1;
            while x < buffer.area.width && cell_background(&buffer[(x, y)]) == background {
                x += 1;
            }
            if background != CANVAS_COLOR {
                let _ = write!(
                    svg,
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
                    start as usize * CELL_WIDTH,
                    y as usize * CELL_HEIGHT,
                    (x - start) as usize * CELL_WIDTH,
                    CELL_HEIGHT,
                    background
                );
            }
        }
    }

    let mut text = String::new();
    for y in 0..buffer.area.height {
        let mut x = 0;
        while x < buffer.area.width {
            let cell = &buffer[(x, y)];
            let symbol = cell.symbol();
            let cell_width = symbol_width(symbol);
            let hidden = cell.modifier.contains(Modifier::HIDDEN);
            let reversed = cell.modifier.contains(Modifier::REVERSED);
            if !hidden && !symbol.trim().is_empty() {
                let foreground = if reversed {
                    resolve_color(cell.bg, false)
                } else {
                    resolve_color(cell.fg, false)
                };
                let escaped = escape_xml(symbol);
                text.push_str(symbol);
                let classes = modifier_classes(cell.modifier);
                let mut style = String::new();
                if cell.modifier.contains(Modifier::BOLD) {
                    style.push_str("font-weight:700;");
                }
                if cell.modifier.contains(Modifier::ITALIC) {
                    style.push_str("font-style:italic;");
                }
                if cell.modifier.contains(Modifier::UNDERLINED)
                    && cell.modifier.contains(Modifier::CROSSED_OUT)
                {
                    style.push_str("text-decoration:underline line-through;");
                } else if cell.modifier.contains(Modifier::UNDERLINED) {
                    style.push_str("text-decoration:underline;");
                } else if cell.modifier.contains(Modifier::CROSSED_OUT) {
                    style.push_str("text-decoration:line-through;");
                }
                if cell.modifier.contains(Modifier::DIM) {
                    style.push_str("opacity:.62;");
                }
                let _ = write!(
                    svg,
                    "<text x=\"{}\" y=\"{}\" fill=\"{}\"",
                    x as usize * CELL_WIDTH,
                    y as usize * CELL_HEIGHT + 14,
                    foreground
                );
                if !style.is_empty() {
                    let _ = write!(svg, " style=\"{}\"", escape_xml(&style));
                }
                if !classes.is_empty() {
                    let _ = write!(svg, " class=\"{}\"", escape_xml(&classes));
                }
                if cell_width > 1 {
                    let _ = write!(
                        svg,
                        " textLength=\"{}\" lengthAdjust=\"spacingAndGlyphs\"",
                        cell_width * CELL_WIDTH
                    );
                }
                let _ = writeln!(svg, ">{escaped}</text>");
            }
            x += cell_width.max(1) as u16;
        }
        text.push('\n');
    }

    svg.push_str("</svg>\n");
    assert_eq!(text.lines().count(), buffer.area.height as usize);
    (svg, text)
}

fn modifier_classes(modifiers: Modifier) -> String {
    let mut classes = String::new();
    if modifiers.contains(Modifier::SLOW_BLINK) {
        classes.push_str("blink");
    }
    if modifiers.contains(Modifier::RAPID_BLINK) {
        if !classes.is_empty() {
            classes.push(' ');
        }
        classes.push_str("rapid-blink");
    }
    classes
}

fn cell_background(cell: &Cell) -> String {
    let color = if cell.modifier.contains(Modifier::REVERSED) {
        cell.fg
    } else {
        cell.bg
    };
    resolve_color(color, true)
}

fn resolve_color(color: Color, background: bool) -> String {
    match color {
        Color::Reset if background => CANVAS_COLOR.to_string(),
        Color::Reset => DEFAULT_FOREGROUND.to_string(),
        Color::Black => "#1c1f24".to_string(),
        Color::Red => "#e06c75".to_string(),
        Color::Green => "#98c379".to_string(),
        Color::Yellow => "#e5c07b".to_string(),
        Color::Blue => "#61afef".to_string(),
        Color::Magenta => "#c678dd".to_string(),
        Color::Cyan => "#56b6c2".to_string(),
        Color::Gray => "#abb2bf".to_string(),
        Color::DarkGray => "#5c6370".to_string(),
        Color::LightRed => "#ff7a85".to_string(),
        Color::LightGreen => "#b6e890".to_string(),
        Color::LightYellow => "#f4d78c".to_string(),
        Color::LightBlue => "#82c7ff".to_string(),
        Color::LightMagenta => "#df8df3".to_string(),
        Color::LightCyan => "#78dce8".to_string(),
        Color::White => "#f4f7fb".to_string(),
        Color::Rgb(red, green, blue) => format!("#{red:02x}{green:02x}{blue:02x}"),
        Color::Indexed(index) => indexed_color(index),
    }
}

fn indexed_color(index: u8) -> String {
    const BASE: [&str; 16] = [
        "#1c1f24", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
        "#5c6370", "#ff7a85", "#b6e890", "#f4d78c", "#82c7ff", "#df8df3", "#78dce8", "#f4f7fb",
    ];
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let (red, green, blue) = match index {
        0..=15 => return BASE[index as usize].to_string(),
        16..=231 => {
            let cube = index - 16;
            (
                LEVELS[(cube / 36) as usize],
                LEVELS[((cube / 6) % 6) as usize],
                LEVELS[(cube % 6) as usize],
            )
        }
        _ => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    };
    format!("#{red:02x}{green:02x}{blue:02x}")
}

fn symbol_width(symbol: &str) -> usize {
    Span::raw(symbol).width().max(1)
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn assert_export_privacy(text: &str, artifact: &str) -> Result<()> {
    let terms = privacy_terms();
    if let Some(name) = privacy_violation(text, &terms) {
        bail!("{artifact} contains a value from environment variable {name}");
    }
    let lowercase = text.to_lowercase();
    let current_dir = env::current_dir().context("failed to read current directory")?;
    if lowercase.contains(&current_dir.to_string_lossy().to_lowercase()) {
        bail!("{artifact} contains the current working directory");
    }
    for pattern in [
        "ghp_",
        "github_pat_",
        "sk-proj-",
        "sk-ant-",
        "AKIA",
        "BEGIN PRIVATE KEY",
        "password=",
        "secret=",
    ] {
        if lowercase.contains(&pattern.to_lowercase()) {
            bail!("{artifact} contains a secret-like token");
        }
    }
    Ok(())
}

fn privacy_violation<'a>(text: &str, terms: &'a [(String, String)]) -> Option<&'a str> {
    let lowercase = text.to_lowercase();
    terms
        .iter()
        .find(|(_, value)| lowercase.contains(&value.to_lowercase()))
        .map(|(name, _)| name.as_str())
}

fn privacy_terms() -> Vec<(String, String)> {
    const IDENTITY_VARIABLES: &[&str] = &[
        "USER", "LOGNAME", "USERNAME", "HOSTNAME", "HOST", "HOME", "PWD", "OLDPWD",
    ];
    let mut terms = Vec::new();
    for (name, value) in env::vars() {
        let sensitive_name = IDENTITY_VARIABLES.contains(&name.as_str())
            || [
                "TOKEN",
                "SECRET",
                "PASSWORD",
                "PASSWD",
                "API_KEY",
                "PRIVATE_KEY",
                "CREDENTIAL",
            ]
            .iter()
            .any(|part| name.contains(part));
        if sensitive_name && value.chars().count() >= 4 {
            terms.push((name, value));
        }
    }
    terms
}

fn self_check() {
    assert_eq!(
        escape_xml("<tag attr=\"a\">&'"),
        "&lt;tag attr=&quot;a&quot;&gt;&amp;&apos;"
    );

    assert_eq!(
        format!("<text>{}</text>", escape_xml("<synthetic> & demo")),
        "<text>&lt;synthetic&gt; &amp; demo</text>"
    );

    let mut sample = Buffer::with_lines(["A<&日"]);
    sample[(0, 0)].set_style(
        Style::default()
            .fg(Color::Red)
            .bg(Color::Blue)
            .add_modifier(Modifier::REVERSED),
    );
    let (svg, text) = render_svg(&sample);
    assert_eq!(text.trim_end(), "A<&日");
    assert!(svg.contains("fill=\"#61afef\">A</text>"));
    assert!(svg.contains("fill=\"#e06c75\""));
    assert!(svg.contains(">&lt;</text>"));
    assert!(svg.contains(">&amp;</text>"));
    assert!(svg.contains(">日</text>"));
    assert!(svg.contains("textLength=\"18\""));

    let terms = [("USER".to_string(), "sample-user".to_string())];
    assert_eq!(privacy_violation("safe demo", &terms), None);
    assert_eq!(privacy_violation("sample-user", &terms), Some("USER"));
}
