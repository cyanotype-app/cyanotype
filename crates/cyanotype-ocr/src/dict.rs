//! The character dictionary of a recognition model: one character per line (`.txt`), or the `character_dict:` list inside the
//! model's `inference.yml` (what PaddlePaddle publishes with its ONNX exports). The yml is read with a deliberately small
//! parser: only that list, whose entries are `  - x`, `  - 'x'` or `  - "x"`.
use anyhow::{Context, Result, bail};

pub fn load(path: &std::path::Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let is_yml = path.extension().is_some_and(|e| e == "yml" || e == "yaml");
    if is_yml { parse_yml(&text) } else { Ok(parse_txt(&text)) }
}

pub fn parse_txt(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .map(str::to_string)
        // The file ends with a newline: that is not an entry. A line holding a single space is the space character.
        .collect::<Vec<_>>()
        .into_iter()
        .enumerate()
        .filter(|(_, l)| !l.is_empty())
        .map(|(_, l)| l)
        .collect()
}

pub fn parse_yml(text: &str) -> Result<Vec<String>> {
    let mut lines = text.lines();
    let mut indent = None;
    for l in lines.by_ref() {
        if l.trim() == "character_dict:" {
            indent = Some(l.len() - l.trim_start().len());
            break;
        }
    }
    let Some(indent) = indent else {
        bail!("no character_dict in the model's yml")
    };
    let mut out = Vec::new();
    for l in lines {
        let ind = l.len() - l.trim_start().len();
        let body = l.trim_start();
        // The list's entries are at the key's own indentation or deeper, as `- value`.
        let Some(v) = body.strip_prefix("- ").or_else(|| (body == "-").then_some("")) else {
            break;
        };
        if ind < indent {
            break;
        }
        out.push(scalar(v)?);
    }
    if out.is_empty() {
        bail!("character_dict is empty");
    }
    Ok(out)
}

fn scalar(v: &str) -> Result<String> {
    if let Some(rest) = v.strip_prefix('\'') {
        let inner = rest
            .strip_suffix('\'')
            .with_context(|| format!("unterminated quote in {v:?}"))?;
        return Ok(inner.replace("''", "'"));
    }
    if let Some(rest) = v.strip_prefix('"') {
        let inner = rest
            .strip_suffix('"')
            .with_context(|| format!("unterminated quote in {v:?}"))?;
        let mut out = String::new();
        let mut it = inner.chars();
        while let Some(c) = it.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('/') => out.push('/'),
                Some('u') => {
                    let hex: String = it.by_ref().take(4).collect();
                    let cp = u32::from_str_radix(&hex, 16).with_context(|| format!("bad escape in {v:?}"))?;
                    out.push(char::from_u32(cp).with_context(|| format!("bad code point in {v:?}"))?);
                }
                Some(o) => bail!("unsupported escape \\{o} in {v:?}"),
                None => bail!("dangling backslash in {v:?}"),
            }
        }
        return Ok(out);
    }
    // A plain scalar: YAML would trim it; the dictionary's entries are single characters.
    Ok(v.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yml_list_with_every_quoting_style() {
        let yml = "PostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - '!'\n  - '\"'\n  - ''''\n  - \"\\\\\"\n  - a\n  - '\u{4e2d}'\n  - \"\\u00e9\"\n  - ' '\nOther: 1\n";
        let d = parse_yml(yml).unwrap();
        assert_eq!(d, vec!["!", "\"", "'", "\\", "a", "\u{4e2d}", "\u{e9}", " "]);
    }

    #[test]
    fn yml_without_the_list_is_an_error() {
        assert!(parse_yml("a: 1\n").is_err());
        assert!(parse_yml("  character_dict:\nb: 2\n").is_err());
    }

    #[test]
    fn txt_keeps_a_space_entry_and_drops_the_trailing_newline() {
        assert_eq!(parse_txt("a\nb\n \nc\n"), vec!["a", "b", " ", "c"]);
        assert_eq!(parse_txt("a\r\nb\r\n"), vec!["a", "b"]);
    }
}
