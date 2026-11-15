//! The line cut a dropped `| head -n N` / `| tail -n N` asked for. When no
//! adapter report replaces the filter (the report would not pay, or the
//! adapter output could not be parsed), `cartoon -c` applies the cut in
//! process to what it emits, so the output is never larger than the agent
//! asked for. The raw log keeps every line.

/// `Head(n)`: the first `n` lines; `Tail(n)`: the last `n` lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineCut {
    Head(usize),
    Tail(usize),
}

impl LineCut {
    /// Parse a dropped filter. Only `head`/`tail` with exactly one line
    /// count (`-n N`, `-nN`, `-N`, `--lines=N`) is a cut; anything else
    /// (`tail`, `xcbeautify`, `head -c 5`) is None.
    pub fn parse(filter: &str) -> Option<LineCut> {
        let words: Vec<&str> = filter.split_whitespace().collect();
        let (name, args) = words.split_first()?;
        let count = match args {
            ["-n", n] => *n,
            [a] => a
                .strip_prefix("--lines=")
                .or_else(|| a.strip_prefix("-n"))
                .or_else(|| a.strip_prefix('-'))?,
            _ => return None,
        };
        if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n = count.parse().ok()?;
        match *name {
            "head" => Some(LineCut::Head(n)),
            "tail" => Some(LineCut::Tail(n)),
            _ => None,
        }
    }

    /// The kept lines of `bytes`, newlines included, as `head`/`tail` would
    /// print them.
    pub fn apply<'a>(&self, bytes: &'a [u8]) -> &'a [u8] {
        let newline_ends: Vec<usize> = bytes
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i + 1)
            .collect();
        match *self {
            LineCut::Head(n) => match n.checked_sub(1).and_then(|i| newline_ends.get(i)) {
                Some(&end) => &bytes[..end],
                None if n == 0 => &bytes[..0],
                None => bytes,
            },
            LineCut::Tail(n) => {
                // A last line without a newline is a line too.
                let unterminated = bytes.last().is_some_and(|b| *b != b'\n');
                let lines = newline_ends.len() + usize::from(unterminated);
                if n >= lines {
                    return bytes;
                }
                if n == 0 {
                    return &bytes[bytes.len()..];
                }
                let skip = lines - n;
                &bytes[newline_ends[skip - 1]..]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_hook_count_forms_only() {
        assert_eq!(LineCut::parse("tail -40"), Some(LineCut::Tail(40)));
        assert_eq!(LineCut::parse("tail -n 5"), Some(LineCut::Tail(5)));
        assert_eq!(LineCut::parse("head -n3"), Some(LineCut::Head(3)));
        assert_eq!(LineCut::parse("head --lines=2"), Some(LineCut::Head(2)));
        for f in [
            "tail",
            "head -c 5",
            "tail -n +5",
            "xcbeautify",
            "tail -f",
            "cat -5",
        ] {
            assert_eq!(LineCut::parse(f), None, "{f}");
        }
    }

    #[test]
    fn head_keeps_the_first_lines() {
        assert_eq!(LineCut::Head(2).apply(b"a\nb\nc\n"), b"a\nb\n");
        assert_eq!(LineCut::Head(5).apply(b"a\nb"), b"a\nb");
        assert_eq!(LineCut::Head(0).apply(b"a\n"), b"");
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        assert_eq!(LineCut::Tail(1).apply(b"a\nb\nc\n"), b"c\n");
        assert_eq!(LineCut::Tail(2).apply(b"a\nb\nc"), b"b\nc");
        assert_eq!(LineCut::Tail(9).apply(b"a\nb\n"), b"a\nb\n");
        assert_eq!(LineCut::Tail(1).apply(b""), b"");
        assert_eq!(LineCut::Tail(0).apply(b"a\nb"), b"");
    }
}
