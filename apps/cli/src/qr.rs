//! A QR code drawn in the terminal with Unicode half blocks (two rows of modules per line), for the device login: the
//! person scans the activation address with a phone instead of typing it.
use qrcode::{Color, EcLevel, QrCode};

/// Quiet zone around the code, in modules (the standard asks for 4; scanners cope with 2 and a terminal has little room).
const QUIET: usize = 2;
/// Black on white whatever the terminal's own colors are: a scanner needs dark modules on a light ground.
const START: &str = "\x1b[30;47m";
const RESET: &str = "\x1b[0m";

/// The code for `text` as lines of text ready to print (colored with escape sequences), or None when it does not fit.
pub fn render(text: &str) -> Option<String> {
    let code = QrCode::with_error_correction_level(text, EcLevel::L).ok()?;
    Some(draw(&code.to_colors(), code.width()))
}

fn draw(colors: &[Color], width: usize) -> String {
    let side = width + 2 * QUIET;
    let dark = |x: usize, y: usize| {
        (QUIET..QUIET + width).contains(&x)
            && (QUIET..QUIET + width).contains(&y)
            && colors[(y - QUIET) * width + (x - QUIET)] == Color::Dark
    };
    let mut out = String::new();
    for y in (0..side).step_by(2) {
        out.push_str(START);
        for x in 0..side {
            out.push(match (dark(x, y), dark(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push_str(RESET);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> Vec<String> {
        s.lines().map(|l| l.replace(START, "").replace(RESET, "")).collect()
    }

    #[test]
    fn a_small_address_makes_a_square_with_a_quiet_zone() {
        let art = render("https://photos.example.com/activate#code=WDJB-MJHT").unwrap();
        let lines = plain(&art);
        assert!(lines.iter().all(|l| l.chars().count() == lines[0].chars().count()));
        let cols = lines[0].chars().count();
        assert_eq!(lines.len(), cols.div_ceil(2));
        // The first line is the quiet zone: blank at both ends, then the top of the finder patterns.
        assert!(lines[0].starts_with("  ") && lines[0].ends_with("  "));
        assert!(art.lines().all(|l| l.starts_with(START) && l.ends_with(RESET)));
    }

    #[test]
    fn half_blocks_follow_the_modules() {
        // 2x2 code: dark top-left and bottom-right, inside a quiet zone of light modules.
        let colors = [Color::Dark, Color::Light, Color::Light, Color::Dark];
        let lines = plain(&draw(&colors, 2));
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].trim(), "");
        assert_eq!(lines[1], "  ▀▄  ");
        assert_eq!(lines[2].trim(), "");
    }

    #[test]
    fn text_too_long_for_any_code_gives_none() {
        assert!(render(&"x".repeat(5000)).is_none());
    }
}
