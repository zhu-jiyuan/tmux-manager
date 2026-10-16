//! Terminal colour and width helpers.
//!
//! fzf rows and the preview are ANSI-coloured text. To align columns and clip
//! captured screens we must measure what the terminal will *show*, which means
//! skipping escape sequences and counting wide (CJK) characters as two cells.

use unicode_width::UnicodeWidthChar;

/// Length in bytes of the CSI escape sequence starting at `s`, if there is one.
///
/// A CSI sequence is `ESC [`, parameter bytes (0x30–0x3F), intermediate bytes
/// (0x20–0x2F), and one final byte (0x40–0x7E). Parsing by hand keeps
/// clipping linear and avoids a regex per character.
fn csi_len(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[0] != 0x1b || bytes[1] != b'[' {
        return None;
    }
    let mut i = 2;
    while i < bytes.len() && (0x30..=0x3f).contains(&bytes[i]) {
        i += 1;
    }
    while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
        i += 1;
    }
    if i < bytes.len() && (0x40..=0x7e).contains(&bytes[i]) {
        Some(i + 1)
    } else {
        None
    }
}

/// Length in bytes of the OSC sequence starting at `s`, if there is one:
/// `ESC ]`, its text, then BEL or `ESC \`. `capture-pane -e` keeps the OSC 8
/// hyperlinks agents print, whose URLs take no cells on screen.
fn osc_len(s: &str) -> Option<usize> {
    let body = s.strip_prefix("\x1b]")?;
    let end = body.find(['\x07', '\x1b'])?;
    match &body[end..] {
        rest if rest.starts_with('\x07') => Some(2 + end + 1),
        rest if rest.starts_with("\x1b\\") => Some(2 + end + 2),
        _ => None,
    }
}

/// The foreground colours tm paints with: the plain ANSI ones, never the
/// bright variants (see `rows::dots` for why).
#[derive(Clone, Copy)]
pub enum Colour {
    Green = 32,
    Yellow = 33,
    Blue = 34,
}

/// `text` in a foreground colour, reset afterwards.
pub fn tint(text: &str, colour: Colour) -> String {
    format!("\x1b[{}m{text}\x1b[0m", colour as u8)
}

/// `text` in bold, reset afterwards.
pub fn bold(text: &str) -> String {
    format!("\x1b[1m{text}\x1b[0m")
}

/// `text` faint (SGR 2), reset afterwards. Secondary text is dimmed as an
/// attribute rather than painted a colour: no palette index is "dim" under
/// every theme (Solarized's brightblack is the darkest tone in light mode
/// and the background in dark mode), but every terminal knows how to fade
/// its own foreground.
pub fn faint(text: &str) -> String {
    format!("\x1b[2m{text}\x1b[0m")
}

fn cell_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// The text as the terminal reads it: escape sequences and characters, in order.
enum Piece<'a> {
    Escape(&'a str),
    Char(char),
}

fn pieces(text: &str) -> impl Iterator<Item = Piece<'_>> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if let Some(len) = csi_len(rest).or_else(|| osc_len(rest)) {
            let (escape, tail) = rest.split_at(len);
            rest = tail;
            return Some(Piece::Escape(escape));
        }
        let c = rest.chars().next()?;
        rest = &rest[c.len_utf8()..];
        Some(Piece::Char(c))
    })
}

/// Remove escape sequences.
pub fn strip(text: &str) -> String {
    pieces(text)
        .filter_map(|piece| match piece {
            Piece::Char(c) => Some(c),
            Piece::Escape(_) => None,
        })
        .collect()
}

/// Number of terminal cells `text` occupies.
pub fn visible_width(text: &str) -> usize {
    pieces(text)
        .map(|piece| match piece {
            Piece::Char(c) => cell_width(c),
            Piece::Escape(_) => 0,
        })
        .sum()
}

/// Pad with spaces on the right up to `width` cells.
pub fn pad(text: &str, width: usize) -> String {
    let fill = width.saturating_sub(visible_width(text));
    format!("{text}{}", " ".repeat(fill))
}

/// Clip one coloured row to `columns` cells without letting it wrap, keeping
/// escape sequences intact and ending with a reset when any were kept.
pub fn clip_line(line: &str, columns: usize) -> String {
    let mut out = String::new();
    let mut width = 0;
    for piece in pieces(line) {
        match piece {
            Piece::Escape(escape) => out.push_str(escape),
            Piece::Char(c) => {
                let w = cell_width(c);
                if width + w > columns {
                    break;
                }
                out.push(c);
                width += w;
            }
        }
    }
    if out.contains('\x1b') {
        out.push_str("\x1b[0m");
    }
    out
}

/// The colours and link in force at a point of a capture. `capture-pane -e`
/// writes an attribute only where it changes, even across lines, so a row
/// cut out of the middle needs them written again to look the same.
#[derive(Default)]
struct Style<'a> {
    /// SGR sequences since the last reset; later ones override earlier.
    sgr: Vec<&'a str>,
    /// The OSC 8 sequence that opened the current hyperlink.
    link: Option<&'a str>,
}

impl<'a> Style<'a> {
    fn apply(&mut self, escape: &'a str) {
        if let Some(params) = escape
            .strip_prefix("\x1b[")
            .and_then(|e| e.strip_suffix('m'))
        {
            if params.is_empty() || params == "0" || params.starts_with("0;") {
                self.sgr.clear();
            }
            if !params.is_empty() && params != "0" {
                self.sgr.push(escape);
            }
        } else if let Some(body) = escape.strip_prefix("\x1b]8;") {
            // `8;params;uri`: an empty URI closes the link.
            let uri = body.split_once(';').map_or("", |(_, uri)| uri);
            let uri = uri.trim_end_matches(['\x07', '\x1b', '\\']);
            self.link = (!uri.is_empty()).then_some(escape);
        }
    }

    /// What starts a row so it looks as it would on the screen.
    fn open(&self) -> String {
        let mut out: String = self.sgr.concat();
        out.extend(self.link);
        out
    }

    /// What ends a row so nothing leaks past it: the link closed, the
    /// attributes reset.
    fn close(&self, row: &mut String) {
        if self.link.is_some() {
            row.push_str("\x1b]8;;\x1b\\");
        }
        if row.contains('\x1b') {
            row.push_str("\x1b[0m");
        }
    }
}

/// Box-drawing and block characters only: a rule or a frame edge, which
/// would wrap into rows of nothing, so it is clipped instead.
fn decoration(text: &str) -> bool {
    text.chars()
        .all(|c| c == ' ' || ('\u{2500}'..='\u{259f}').contains(&c))
}

/// A stretch that wraps as a whole: a word, one space, or one wide
/// character, which may break from its neighbours on either side, each with
/// the escapes in front of it.
struct Unit<'a> {
    pieces: Vec<Piece<'a>>,
    width: usize,
    space: bool,
}

/// `line` as units up to `content` cells, past which are only trailing
/// blanks, and the escapes left over after the last unit.
fn units(line: &str, content: usize) -> (Vec<Unit<'_>>, Vec<&str>) {
    let mut units: Vec<Unit> = Vec::new();
    let mut escapes = Vec::new();
    let mut seen = 0;
    // Whether the last unit is a word a narrow character extends.
    let mut in_word = false;
    for piece in pieces(line) {
        let c = match piece {
            Piece::Escape(escape) => {
                escapes.push(escape);
                continue;
            }
            Piece::Char(c) => c,
        };
        let w = cell_width(c);
        seen += w;
        if seen > content {
            continue;
        }
        let space = c == ' ';
        let narrow = !space && w <= 1;
        if !(narrow && in_word) {
            units.push(Unit {
                pieces: Vec::new(),
                width: 0,
                space,
            });
        }
        if let Some(unit) = units.last_mut() {
            unit.pieces.extend(escapes.drain(..).map(Piece::Escape));
            unit.pieces.push(Piece::Char(c));
            unit.width += w;
        }
        in_word = narrow;
    }
    (units, escapes)
}

/// The rows one line is laid out into.
struct Layout<'s, 'a> {
    style: &'s mut Style<'a>,
    rows: Vec<String>,
    row: String,
    width: usize,
    indent: usize,
}

impl<'a> Layout<'_, 'a> {
    fn escape(&mut self, escape: &'a str) {
        self.style.apply(escape);
        self.row.push_str(escape);
    }

    fn char(&mut self, c: char) {
        self.row.push(c);
        self.width += cell_width(c);
    }

    /// End the row, without the spaces it broke after, and start the next
    /// under the indent.
    fn break_row(&mut self) {
        let kept = self.row.trim_end_matches(' ').len();
        self.row.truncate(kept);
        self.style.close(&mut self.row);
        let next = format!("{}{}", self.style.open(), " ".repeat(self.indent));
        self.rows.push(std::mem::replace(&mut self.row, next));
        self.width = self.indent;
    }

    fn finish(mut self) -> Vec<String> {
        self.style.close(&mut self.row);
        self.rows.push(self.row);
        self.rows
    }
}

/// One line of a capture as rows of at most `columns` cells. Text wraps at
/// spaces and around wide characters, under its own indent, and only a word
/// longer than a row splits; trailing blanks never start a row, and a
/// decoration line is clipped. `style` carries over from line to line.
fn wrap<'a>(line: &'a str, columns: usize, style: &mut Style<'a>) -> Vec<String> {
    let text = strip(line);
    // Up to a third of the row, so a deep indent still leaves room for text.
    let indent = (text.len() - text.trim_start_matches(' ').len()).min(columns / 3);
    let row = style.open();
    let mut out = Layout {
        style,
        rows: Vec::new(),
        row,
        width: 0,
        indent,
    };
    if decoration(&text) {
        for piece in pieces(line) {
            match piece {
                Piece::Escape(escape) => out.escape(escape),
                Piece::Char(c) if out.width + cell_width(c) <= columns => out.char(c),
                Piece::Char(_) => {}
            }
        }
        return out.finish();
    }
    let (units, tail) = units(line, visible_width(text.trim_end()));
    for unit in units {
        // A row holding no more than its indent takes what comes, so a word
        // longer than a row splits rather than leave an empty row behind.
        let mut skip = false;
        if out.width + unit.width > columns && out.width > indent && unit.width <= columns - indent
        {
            out.break_row();
            // The space a row broke at would only push the next one in.
            skip = unit.space;
        }
        for piece in unit.pieces {
            match piece {
                Piece::Escape(escape) => out.escape(escape),
                Piece::Char(_) if skip => {}
                Piece::Char(c) => {
                    if out.width + cell_width(c) > columns && out.width > indent {
                        out.break_row();
                    }
                    out.char(c);
                }
            }
        }
    }
    for escape in tail {
        out.escape(escape);
    }
    out.finish()
}

/// The bottom of a screen capture as `lines` rows of at most `columns`
/// cells: long lines wrap rather than lose their ends, runs of blank lines
/// shrink to one, and trailing blank lines go, so the narrow preview shows
/// as much of the latest output as it can.
pub fn fit_frame(frame: &str, columns: usize, lines: usize) -> String {
    let mut style = Style::default();
    let mut rows: Vec<String> = Vec::new();
    for line in frame.lines() {
        let blank = strip(line).trim().is_empty();
        let wrapped = wrap(line, columns, &mut style);
        if !blank {
            rows.extend(wrapped);
        } else if rows.last().is_some_and(|row| !strip(row).trim().is_empty()) {
            rows.push(String::new());
        }
    }
    while rows.last().is_some_and(|row| strip(row).trim().is_empty()) {
        rows.pop();
    }
    let start = rows.len().saturating_sub(lines);
    rows[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_the_plain_ansi_ones() {
        assert_eq!(tint("x", Colour::Green), "\x1b[32mx\x1b[0m");
        assert_eq!(tint("x", Colour::Yellow), "\x1b[33mx\x1b[0m");
        assert_eq!(tint("x", Colour::Blue), "\x1b[34mx\x1b[0m");
    }

    #[test]
    fn width_ignores_escapes_and_counts_wide_chars() {
        assert_eq!(visible_width(&tint("ab", Colour::Blue)), 2);
        assert_eq!(visible_width("界z"), 3);
        assert_eq!(pad("ab", 4), "ab  ");
    }

    #[test]
    fn frames_wrap_and_follow_the_bottom() {
        assert_eq!(
            fit_frame("top\n0123456789ABCD\ncurrent\nlast\n\n", 8, 4),
            "01234567\n89ABCD\ncurrent\nlast",
            "a long line wraps, the bottom stays"
        );
        assert_eq!(
            fit_frame("  one two three", 9, 9),
            "  one two\n  three",
            "under its own indent"
        );
        assert_eq!(
            fit_frame("say hello world, 你好世界", 8, 9),
            "say\nhello\nworld,\n你好世界",
            "words move whole; CJK breaks anywhere"
        );
        assert_eq!(
            fit_frame("abcdef界z", 7, 3),
            "abcdef\n界z",
            "a wide character moves whole to the next row"
        );
        assert_eq!(
            fit_frame("one\n\n\n\ntwo   \n\n", 4, 9),
            "one\n\ntwo",
            "blank runs shrink to one, trailing blanks start no row"
        );
        assert_eq!(
            fit_frame("────────────\n├───┼───┤───┤\ntext", 4, 9),
            "────\n├───\ntext",
            "rules are clipped, not wrapped"
        );
        assert_eq!(
            clip_line("\x1b[31mred\x1b[0m tail", 4),
            "\x1b[31mred\x1b[0m \x1b[0m"
        );
    }

    #[test]
    fn frames_carry_colours_and_links_across_rows() {
        assert_eq!(
            fit_frame("\x1b[31mredred\nyes", 3, 9),
            "\x1b[31mred\x1b[0m\n\x1b[31mred\x1b[0m\n\x1b[31myes\x1b[0m",
            "every row restates the colour it starts in"
        );
        assert_eq!(
            fit_frame("\x1b[1m\x1b[0;3mab\x1b[0mcd", 2, 9),
            "\x1b[1m\x1b[0;3mab\x1b[0m\x1b[0m\ncd",
            "a reset forgets what came before"
        );
        let link = "\x1b]8;;https://example.com/a/long/path\x1b\\";
        let close = "\x1b]8;;\x1b\\";
        assert_eq!(visible_width(&format!("{link}site{close}")), 4);
        assert_eq!(
            fit_frame(&format!("{link}sitesite{close}"), 4, 9),
            format!("{link}site{close}\x1b[0m\n{link}site{close}\x1b[0m"),
            "a link is closed at the end of a row and reopened on the next"
        );
    }
}
