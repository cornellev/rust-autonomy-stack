//! Terminal output in CEV team colours: Cornell red (`#B31B1B`, from
//! cornellev.org) and bold type. Colour is dropped automatically when the
//! stream is not a terminal or `NO_COLOR` is set.

use std::fmt::Display;
use std::io::IsTerminal;

use anstyle::{Color, RgbColor, Style};

/// Cornell red.
const RED: Color = Color::Rgb(RgbColor(0xB3, 0x1B, 0x1B));
const WHITE: Color = Color::Rgb(RgbColor(0xFF, 0xFF, 0xFF));

/// White on red, for status labels and car names.
const BADGE: Style = Style::new()
    .fg_color(Some(WHITE))
    .bg_color(Some(RED))
    .bold();
/// Red on white, for errors: the inverse of a badge.
const ERROR_BADGE: Style = Style::new()
    .fg_color(Some(RED))
    .bg_color(Some(WHITE))
    .bold();
/// Red text.
const ACCENT: Style = Style::new().fg_color(Some(RED)).bold();
/// The terminal's own foreground colour, bold, so it reads on dark and light themes.
const STRONG: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();

/// Width of a status label, so the messages after it line up.
const LABEL_WIDTH: usize = 8;

/// ` CHECKING  car sim`: an uppercase label in a red badge, then the message.
pub fn status(verb: &str, message: impl Display) {
    let label = verb.to_uppercase();
    anstream::eprintln!("{BADGE} {label:<LABEL_WIDTH$} {BADGE:#} {message}");
}

/// ` ERROR  ...`, in an inverted badge with the message in red.
pub fn error(message: impl Display) {
    anstream::eprintln!(
        "{ERROR_BADGE} {:<LABEL_WIDTH$} {ERROR_BADGE:#} {ACCENT}{message}{ACCENT:#}",
        "ERROR"
    );
}

/// A car name as a badge, e.g. ` MERCEVES `.
pub fn car_badge(name: &str) -> String {
    format!("{BADGE} {} {BADGE:#}", name.to_uppercase())
}

pub fn accent(text: impl Display) -> String {
    format!("{ACCENT}{text}{ACCENT:#}")
}

pub fn strong(text: impl Display) -> String {
    format!("{STRONG}{text}{STRONG:#}")
}

pub fn dim(text: impl Display) -> String {
    format!("{DIM}{text}{DIM:#}")
}

/// The CEV mark from `ev-website/public/fulllight.png`, as half-block cells:
/// each character is one cell holding two stacked pixels.
///
/// | code | top   | bottom |
/// |------|-------|--------|
/// | `W`  | C     | C      |
/// | `w`  | C     | -      |
/// | `m`  | -     | C      |
/// | `R`  | bolt  | bolt   |
/// | `r`  | bolt  | -      |
/// | `n`  | -     | bolt   |
/// | `x`  | C     | bolt   |
///
/// The C is drawn in the terminal's foreground colour (white on a dark theme,
/// black on a light one); the bolt is Cornell red.
const LOGO: [&str; 21] = [
    "    WWWWWWWWWWWW",
    "   WWW       WWw",
    "   WW        WW",
    "  WWw",
    " mWW",
    " WW   nnnnnnnnnnn",
    "WWWmmmRRrrrrrrRRR",
    "wWWWWRRr      RR",
    "    RRR      rrr",
    "   nRRRRRR",
    "   RRr",
    "  nRR",
    "  RRRRRRR    RRRRRRR",
    "   rrrRRR    rrRRRr",
    "      RR     nRRr",
    "     RRR   nRRRr",
    "     RR   RRRr",
    "    RRR nRRr",
    "    RRRRRr",
    "   nRRRr",
    "   RRR",
];

/// Widest row of [`LOGO`], in cells.
const LOGO_WIDTH: usize = 20;
/// The row of [`LOGO`] the wordmark starts beside.
const WORDMARK_ROW: usize = 8;

/// Render one logo cell.
fn logo_cell(code: char) -> String {
    let c = Style::new();
    let bolt = Style::new().fg_color(Some(RED));
    let (style, glyph) = match code {
        'W' => (c, '█'),
        'w' => (c, '▀'),
        'm' => (c, '▄'),
        'R' => (bolt, '█'),
        'r' => (bolt, '▀'),
        'n' => (bolt, '▄'),
        'x' => (c.bg_color(Some(RED)), '▀'),
        _ => return " ".to_string(),
    };
    format!("{style}{glyph}{style:#}")
}

/// The logo with "CORNELL ELECTRIC VEHICLES" beside it, on stdout. Skipped
/// when stdout is not a terminal, so `cev cars | grep` sees only the list.
pub fn banner(tagline: &str) {
    if !std::io::stdout().is_terminal() {
        return;
    }
    let words = [
        strong("C O R N E L L"),
        accent("E L E C T R I C   V E H I C L E S"),
        String::new(),
        dim(tagline),
    ];
    for (i, row) in LOGO.iter().enumerate() {
        let cells: String = row.chars().map(logo_cell).collect();
        let pad = " ".repeat(LOGO_WIDTH.saturating_sub(row.chars().count()));
        let beside = i
            .checked_sub(WORDMARK_ROW)
            .and_then(|j| words.get(j))
            .map(String::as_str)
            .unwrap_or("");
        anstream::println!("  {cells}{pad}   {beside}");
    }
    anstream::println!();
}
