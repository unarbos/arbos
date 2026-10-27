//! A small LaTeX subset typeset as plain text runs: what `$O(n^2)$` or
//! `\( \sum_{i=1}^{n} i \)` in an agent reply becomes on screen. No layout
//! engine — superscripts and subscripts use Unicode's own small glyphs, a
//! fraction is `a/b`, a root is `√x` — but letters are italic in a math face
//! and digits, operators and function names upright, the way KaTeX paints
//! Grok Bot's replies ([feedback math-render-mac-2026-10-26]).
//!
//! The iPhone app carries the same subset in `UI/MathText.swift`; keep the
//! two in step (the test vectors at the bottom are shared).

/// How one run of typeset math is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathStyle {
    /// Variables: Latin letters and lowercase Greek.
    Italic,
    /// Digits, operators, brackets, function names, `\text{…}`.
    Upright,
    /// `\mathbf{…}` / `\boldsymbol{…}`.
    Bold,
}

/// One run of the typeset text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathRun {
    pub text: String,
    pub style: MathStyle,
}

/// `src` (the LaTeX between the delimiters) as styled runs, adjacent runs of
/// one style merged. Never fails: unknown commands come out as their name.
pub fn typeset(src: &str) -> Vec<MathRun> {
    let mut p = Parser {
        chars: src.chars().collect(),
        i: 0,
        script_depth: 0,
    };
    let mut out = Out::default();
    p.sequence(&mut out, Mode::default(), false);
    out.finish()
}

/// The typeset text with no styling: what a selection copies and what sizes
/// a table column.
pub fn plain(src: &str) -> String {
    typeset(src).into_iter().map(|r| r.text).collect()
}

/// Letter treatment inside the current group.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Math,
    /// `\mathrm{…}`: letters upright.
    Upright,
    /// `\mathbf{…}`: everything bold.
    Bold,
}

#[derive(Default)]
struct Out {
    runs: Vec<MathRun>,
    /// The last thing pushed can take a binary operator after it (an operand,
    /// not an operator or an opening bracket): `a - b` spaces the minus,
    /// `(-1)` does not.
    operand_before: bool,
    /// A function name (`\sin`) was just pushed: a thin space follows unless
    /// a bracket does.
    after_function: bool,
}

impl Out {
    fn push(&mut self, text: &str, style: MathStyle) {
        if text.is_empty() {
            return;
        }
        if self.after_function {
            self.after_function = false;
            if !text.starts_with(['(', '[', '{', '|']) {
                self.raw("\u{2009}", MathStyle::Upright);
            }
        }
        self.raw(text, style);
        self.operand_before = !text.ends_with(['(', '[', '{', ' ', ',', '\u{2009}']);
    }

    fn raw(&mut self, text: &str, style: MathStyle) {
        if let Some(last) = self.runs.last_mut() {
            if last.style == style {
                last.text.push_str(text);
                return;
            }
        }
        self.runs.push(MathRun {
            text: text.to_string(),
            style,
        });
    }

    /// `op` with a space on each side when it sits between two operands
    /// (`a + b`, `x = 1`), bare when it is unary (`-1`) or inside a script.
    fn operator(&mut self, op: &str, spaced: bool) {
        if spaced && self.operand_before {
            self.push(" ", MathStyle::Upright);
            self.push(op, MathStyle::Upright);
            self.push(" ", MathStyle::Upright);
        } else {
            self.push(op, MathStyle::Upright);
        }
        self.operand_before = false;
    }

    fn finish(mut self) -> Vec<MathRun> {
        if let Some(last) = self.runs.last_mut() {
            let trimmed = last.text.trim_end().len();
            last.text.truncate(trimmed);
        }
        if let Some(first) = self.runs.first_mut() {
            let trimmed = first.text.trim_start().to_string();
            first.text = trimmed;
        }
        self.runs.retain(|r| !r.text.is_empty());
        self.runs
    }
}

struct Parser {
    chars: Vec<char>,
    i: usize,
    /// Above zero while typesetting the argument of `^` or `_`: no operator
    /// spacing, nested scripts fall back to `^`/`_` text.
    script_depth: u32,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.i += 1;
        }
    }

    /// Typeset until the end, or until the `}` closing an open group.
    fn sequence(&mut self, out: &mut Out, mode: Mode, in_group: bool) {
        while let Some(c) = self.peek() {
            match c {
                '}' => {
                    if in_group {
                        self.i += 1;
                        return;
                    }
                    self.i += 1;
                }
                '{' => {
                    self.i += 1;
                    self.sequence(out, mode, true);
                }
                '\\' => self.command(out, mode),
                '^' | '_' => {
                    self.i += 1;
                    let arg = self.script_argument(mode);
                    self.script(out, c == '^', &arg);
                }
                c if c.is_whitespace() => {
                    self.i += 1;
                }
                '&' => {
                    self.i += 1;
                    out.push(" ", MathStyle::Upright);
                }
                _ => {
                    self.i += 1;
                    self.symbol(out, c, mode);
                }
            }
        }
    }

    fn letter_style(mode: Mode) -> MathStyle {
        match mode {
            Mode::Math => MathStyle::Italic,
            Mode::Upright => MathStyle::Upright,
            Mode::Bold => MathStyle::Bold,
        }
    }

    fn fixed_style(mode: Mode) -> MathStyle {
        match mode {
            Mode::Math | Mode::Upright => MathStyle::Upright,
            Mode::Bold => MathStyle::Bold,
        }
    }

    fn spaced(&self) -> bool {
        self.script_depth == 0
    }

    /// One plain character of the source.
    fn symbol(&mut self, out: &mut Out, c: char, mode: Mode) {
        let fixed = Self::fixed_style(mode);
        match c {
            c if c.is_alphabetic() => out.push(&c.to_string(), Self::letter_style(mode)),
            '-' => out.operator("\u{2212}", self.spaced()),
            '+' => out.operator("+", self.spaced()),
            '=' | '<' | '>' => out.operator(&c.to_string(), self.spaced()),
            '*' => out.operator("\u{2217}", self.spaced()),
            '\'' => out.push("\u{2032}", fixed),
            ',' | ';' => {
                out.push(&c.to_string(), fixed);
                if self.spaced() {
                    out.push(" ", fixed);
                }
            }
            _ => out.push(&c.to_string(), fixed),
        }
    }

    /// `\name` or `\<one char>`: the name without the backslash.
    fn command_name(&mut self) -> String {
        self.i += 1;
        let mut name = String::new();
        match self.peek() {
            Some(c) if c.is_ascii_alphabetic() => {
                while let Some(c) = self.peek() {
                    if !c.is_ascii_alphabetic() {
                        break;
                    }
                    name.push(c);
                    self.i += 1;
                }
            }
            Some(c) => {
                name.push(c);
                self.i += 1;
            }
            None => {}
        }
        name
    }

    /// The next argument: a `{…}` group, a command, or one character.
    fn argument_src(&mut self) -> String {
        self.skip_spaces();
        match self.peek() {
            Some('{') => {
                self.i += 1;
                let start = self.i;
                let mut depth = 1;
                while let Some(c) = self.peek() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    self.i += 1;
                }
                let src: String = self.chars[start..self.i].iter().collect();
                if self.peek() == Some('}') {
                    self.i += 1;
                }
                src
            }
            Some('\\') => {
                let start = self.i;
                self.command_name();
                self.chars[start..self.i].iter().collect()
            }
            Some(c) => {
                self.i += 1;
                c.to_string()
            }
            None => String::new(),
        }
    }

    fn sub_typeset(&mut self, src: &str, mode: Mode, script: bool) -> Vec<MathRun> {
        let mut p = Parser {
            chars: src.chars().collect(),
            i: 0,
            script_depth: self.script_depth + u32::from(script),
        };
        let mut out = Out::default();
        p.sequence(&mut out, mode, false);
        out.finish()
    }

    fn script_argument(&mut self, mode: Mode) -> Vec<MathRun> {
        let src = self.argument_src();
        self.sub_typeset(&src, mode, true)
    }

    /// A superscript or subscript: Unicode's small glyphs when every
    /// character has one, else `^x` / `_(xy)` in normal text.
    fn script(&mut self, out: &mut Out, up: bool, arg: &[MathRun]) {
        let plain: String = arg.iter().map(|r| r.text.as_str()).collect();
        if plain.is_empty() {
            return;
        }
        let mapped: Option<Vec<MathRun>> = arg
            .iter()
            .map(|r| {
                r.text
                    .chars()
                    .map(|c| if up { superscript(c) } else { subscript(c) })
                    .collect::<Option<String>>()
                    .map(|text| MathRun {
                        text,
                        style: r.style,
                    })
            })
            .collect();
        match mapped {
            Some(runs) if self.script_depth == 0 => {
                // The thin space after `\sum` / `\sin` belongs after the
                // scripts, not before them.
                let after_function = std::mem::take(&mut out.after_function);
                for r in runs {
                    out.push(&r.text, r.style);
                }
                out.after_function = after_function;
            }
            _ => {
                let after_function = std::mem::take(&mut out.after_function);
                out.push(if up { "^" } else { "_" }, MathStyle::Upright);
                let wrap = plain.chars().count() > 1;
                if wrap {
                    out.push("(", MathStyle::Upright);
                }
                for r in arg {
                    out.push(&r.text, r.style);
                }
                if wrap {
                    out.push(")", MathStyle::Upright);
                }
                out.after_function = after_function;
            }
        }
        out.operand_before = true;
    }

    fn command(&mut self, out: &mut Out, mode: Mode) {
        let name = self.command_name();
        let fixed = Self::fixed_style(mode);
        let spaced = self.spaced();
        match name.as_str() {
            "" => {}
            // Spacing.
            "," | ":" | ";" | " " | "quad" | "qquad" | "~" => out.push(" ", fixed),
            "!" => {}
            "\\" => out.push("\n", fixed),
            "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl"
            | "Bigr" | "biggl" | "biggr" | "limits" | "nolimits" | "displaystyle" | "textstyle"
            | "scriptstyle" | "scriptscriptstyle" => {
                if matches!(name.as_str(), "left" | "right") && self.peek() == Some('.') {
                    self.i += 1;
                }
            }
            "{" | "}" | "%" | "$" | "#" | "&" | "_" => out.push(&name, fixed),
            "|" => out.push("\u{2016}", fixed),
            // Text and letter treatment.
            "text" | "textrm" | "operatorname" | "mbox" => {
                let literal = self.argument_src();
                out.push(&literal, fixed);
                if name == "operatorname" {
                    out.after_function = true;
                }
            }
            "textbf" => {
                let literal = self.argument_src();
                out.push(&literal, MathStyle::Bold);
            }
            "textit" | "mathit" => {
                let src = self.argument_src();
                for r in self.sub_typeset(&src, Mode::Math, false) {
                    out.push(&r.text, MathStyle::Italic);
                }
            }
            "mathrm" => {
                let src = self.argument_src();
                for r in self.sub_typeset(&src, Mode::Upright, false) {
                    out.push(&r.text, r.style);
                }
            }
            "mathbf" | "boldsymbol" | "bm" | "pmb" => {
                let src = self.argument_src();
                for r in self.sub_typeset(&src, Mode::Bold, false) {
                    out.push(&r.text, r.style);
                }
            }
            "mathbb" => {
                let src = self.argument_src();
                let text: String = src.chars().map(blackboard).collect();
                out.push(&text, fixed);
            }
            "mathcal" | "mathscr" | "mathfrak" | "mathsf" | "mathtt" => {
                let src = self.argument_src();
                for r in self.sub_typeset(&src, mode, false) {
                    out.push(&r.text, r.style);
                }
            }
            // Structures.
            "frac" | "dfrac" | "tfrac" => {
                let a = self.argument_src();
                let b = self.argument_src();
                self.fraction(out, mode, &a, &b);
            }
            "binom" => {
                let a = self.argument_src();
                let b = self.argument_src();
                out.push("C(", fixed);
                for r in self.sub_typeset(&a, mode, false) {
                    out.push(&r.text, r.style);
                }
                out.push(", ", fixed);
                for r in self.sub_typeset(&b, mode, false) {
                    out.push(&r.text, r.style);
                }
                out.push(")", fixed);
            }
            "sqrt" => {
                let index = if self.peek_after_spaces() == Some('[') {
                    self.skip_spaces();
                    self.i += 1;
                    let start = self.i;
                    while let Some(c) = self.peek() {
                        if c == ']' {
                            break;
                        }
                        self.i += 1;
                    }
                    let idx: String = self.chars[start..self.i].iter().collect();
                    if self.peek() == Some(']') {
                        self.i += 1;
                    }
                    idx
                } else {
                    String::new()
                };
                let src = self.argument_src();
                let radical = match index.trim() {
                    "" | "2" => "\u{221A}".to_string(),
                    "3" => "\u{221B}".to_string(),
                    "4" => "\u{221C}".to_string(),
                    other => {
                        let small: Option<String> = other.chars().map(superscript).collect();
                        format!("{}\u{221A}", small.unwrap_or_else(|| other.to_string()))
                    }
                };
                out.push(&radical, fixed);
                let inner = self.sub_typeset(&src, mode, false);
                let plain: String = inner.iter().map(|r| r.text.as_str()).collect();
                let wrap = plain.chars().count() > 1 && !enclosed(&plain);
                if wrap {
                    out.push("(", fixed);
                }
                for r in inner {
                    out.push(&r.text, r.style);
                }
                if wrap {
                    out.push(")", fixed);
                }
            }
            "hat" | "bar" | "vec" | "dot" | "ddot" | "tilde" | "overline" | "underline"
            | "widehat" | "widetilde" | "overrightarrow" => {
                let mark = match name.as_str() {
                    "hat" | "widehat" => '\u{0302}',
                    "bar" => '\u{0304}',
                    "vec" | "overrightarrow" => '\u{20D7}',
                    "dot" => '\u{0307}',
                    "ddot" => '\u{0308}',
                    "tilde" | "widetilde" => '\u{0303}',
                    "overline" => '\u{0305}',
                    _ => '\u{0332}',
                };
                let src = self.argument_src();
                for r in self.sub_typeset(&src, mode, false) {
                    let text: String = r.text.chars().flat_map(|c| [c, mark]).collect();
                    out.push(&text, r.style);
                }
            }
            // Function names: upright, a thin space before the argument.
            "sin" | "cos" | "tan" | "cot" | "sec" | "csc" | "arcsin" | "arccos" | "arctan"
            | "sinh" | "cosh" | "tanh" | "coth" | "log" | "ln" | "lg" | "exp" | "lim"
            | "limsup" | "liminf" | "max" | "min" | "sup" | "inf" | "det" | "gcd" | "arg"
            | "deg" | "dim" | "ker" | "hom" | "Pr" => {
                if out.operand_before {
                    out.push("\u{2009}", fixed);
                }
                out.push(&name, fixed);
                out.after_function = true;
            }
            // Relations and binary operators: spaced like `=`.
            _ => {
                if let Some(sym) = relation(&name) {
                    out.operator(sym, spaced);
                } else if let Some(sym) = binary(&name) {
                    out.operator(sym, spaced);
                } else if let Some((sym, italic)) = symbol(&name) {
                    let style = if italic && mode == Mode::Math {
                        MathStyle::Italic
                    } else {
                        fixed
                    };
                    out.push(sym, style);
                    if matches!(
                        name.as_str(),
                        "sum" | "prod" | "int" | "iint" | "oint" | "coprod" | "bigcup" | "bigcap"
                    ) {
                        out.after_function = true;
                    }
                } else {
                    out.push(&name, fixed);
                }
            }
        }
    }

    fn peek_after_spaces(&self) -> Option<char> {
        self.chars[self.i..]
            .iter()
            .find(|c| !c.is_whitespace())
            .copied()
    }

    /// `\frac{a}{b}`: `¹⁄₂` for digits, otherwise `a/b` with a side in
    /// parentheses when it has more than one term.
    fn fraction(&mut self, out: &mut Out, mode: Mode, a: &str, b: &str) {
        let fixed = Self::fixed_style(mode);
        let top = self.sub_typeset(a, mode, false);
        let bottom = self.sub_typeset(b, mode, false);
        let top_plain: String = top.iter().map(|r| r.text.as_str()).collect();
        let bottom_plain: String = bottom.iter().map(|r| r.text.as_str()).collect();
        let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
        if digits(&top_plain) && digits(&bottom_plain) {
            let small: String = top_plain
                .chars()
                .filter_map(superscript)
                .chain(std::iter::once('\u{2044}'))
                .chain(bottom_plain.chars().filter_map(subscript))
                .collect();
            out.push(&small, fixed);
            return;
        }
        let side = |out: &mut Out, runs: Vec<MathRun>, plain: &str| {
            let wrap = needs_parens(plain);
            if wrap {
                out.push("(", fixed);
            }
            for r in runs {
                out.push(&r.text, r.style);
            }
            if wrap {
                out.push(")", fixed);
            }
        };
        side(out, top, &top_plain);
        out.push("/", fixed);
        out.operand_before = false;
        side(out, bottom, &bottom_plain);
    }
}

/// A fraction side or a root's content that reads wrong without brackets:
/// more than one term, and not already bracketed as a whole.
fn needs_parens(plain: &str) -> bool {
    if plain.chars().count() <= 1 || enclosed(plain) {
        return false;
    }
    plain.contains([
        ' ', '+', '\u{2212}', '\u{00B1}', '\u{00D7}', '\u{22C5}', '/',
    ])
}

/// `(…)` / `[…]` / `{…}` / `|…|` around the whole string.
fn enclosed(plain: &str) -> bool {
    let mut chars = plain.chars();
    let (Some(first), Some(last)) = (chars.next(), chars.next_back()) else {
        return false;
    };
    let pair = matches!(
        (first, last),
        ('(', ')') | ('[', ']') | ('{', '}') | ('|', '|') | ('\u{2016}', '\u{2016}')
    );
    if !pair {
        return false;
    }
    if first == '|' || first == '\u{2016}' {
        return true;
    }
    let mut depth = 0i32;
    for (idx, c) in plain.chars().enumerate() {
        if c == first {
            depth += 1;
        } else if c == last {
            depth -= 1;
            if depth == 0 && idx + 1 < plain.chars().count() {
                return false;
            }
        }
    }
    depth == 0
}

fn superscript(c: char) -> Option<char> {
    Some(match c {
        '0' => '\u{2070}',
        '1' => '\u{00B9}',
        '2' => '\u{00B2}',
        '3' => '\u{00B3}',
        '4' => '\u{2074}',
        '5' => '\u{2075}',
        '6' => '\u{2076}',
        '7' => '\u{2077}',
        '8' => '\u{2078}',
        '9' => '\u{2079}',
        '+' => '\u{207A}',
        '\u{2212}' | '-' => '\u{207B}',
        '=' => '\u{207C}',
        '(' => '\u{207D}',
        ')' => '\u{207E}',
        'a' => '\u{1D43}',
        'b' => '\u{1D47}',
        'c' => '\u{1D9C}',
        'd' => '\u{1D48}',
        'e' => '\u{1D49}',
        'f' => '\u{1DA0}',
        'g' => '\u{1D4D}',
        'h' => '\u{02B0}',
        'i' => '\u{2071}',
        'j' => '\u{02B2}',
        'k' => '\u{1D4F}',
        'l' => '\u{02E1}',
        'm' => '\u{1D50}',
        'n' => '\u{207F}',
        'o' => '\u{1D52}',
        'p' => '\u{1D56}',
        'r' => '\u{02B3}',
        's' => '\u{02E2}',
        't' => '\u{1D57}',
        'u' => '\u{1D58}',
        'v' => '\u{1D5B}',
        'w' => '\u{02B7}',
        'x' => '\u{02E3}',
        'y' => '\u{02B8}',
        'z' => '\u{1DBB}',
        'T' => '\u{1D40}',
        'N' => '\u{1D3A}',
        '\u{2032}' => '\u{2032}',
        '\u{00B0}' => '\u{00B0}',
        _ => return None,
    })
}

fn subscript(c: char) -> Option<char> {
    Some(match c {
        '0' => '\u{2080}',
        '1' => '\u{2081}',
        '2' => '\u{2082}',
        '3' => '\u{2083}',
        '4' => '\u{2084}',
        '5' => '\u{2085}',
        '6' => '\u{2086}',
        '7' => '\u{2087}',
        '8' => '\u{2088}',
        '9' => '\u{2089}',
        '+' => '\u{208A}',
        '\u{2212}' | '-' => '\u{208B}',
        '=' => '\u{208C}',
        '(' => '\u{208D}',
        ')' => '\u{208E}',
        'a' => '\u{2090}',
        'e' => '\u{2091}',
        'h' => '\u{2095}',
        'i' => '\u{1D62}',
        'j' => '\u{2C7C}',
        'k' => '\u{2096}',
        'l' => '\u{2097}',
        'm' => '\u{2098}',
        'n' => '\u{2099}',
        'o' => '\u{2092}',
        'p' => '\u{209A}',
        'r' => '\u{1D63}',
        's' => '\u{209B}',
        't' => '\u{209C}',
        'u' => '\u{1D64}',
        'v' => '\u{1D65}',
        'x' => '\u{2093}',
        _ => return None,
    })
}

fn blackboard(c: char) -> char {
    match c {
        'R' => '\u{211D}',
        'N' => '\u{2115}',
        'Z' => '\u{2124}',
        'Q' => '\u{211A}',
        'C' => '\u{2102}',
        'P' => '\u{2119}',
        'H' => '\u{210D}',
        other => other,
    }
}

/// Relations (`=`-like spacing).
fn relation(name: &str) -> Option<&'static str> {
    Some(match name {
        "leq" | "le" | "leqslant" => "\u{2264}",
        "geq" | "ge" | "geqslant" => "\u{2265}",
        "neq" | "ne" => "\u{2260}",
        "approx" => "\u{2248}",
        "equiv" => "\u{2261}",
        "sim" => "\u{223C}",
        "simeq" => "\u{2243}",
        "cong" => "\u{2245}",
        "propto" => "\u{221D}",
        "ll" => "\u{226A}",
        "gg" => "\u{226B}",
        "to" | "rightarrow" => "\u{2192}",
        "leftarrow" | "gets" => "\u{2190}",
        "leftrightarrow" => "\u{2194}",
        "Rightarrow" | "implies" => "\u{21D2}",
        "Leftarrow" => "\u{21D0}",
        "Leftrightarrow" | "iff" => "\u{21D4}",
        "mapsto" => "\u{21A6}",
        "in" => "\u{2208}",
        "notin" => "\u{2209}",
        "ni" => "\u{220B}",
        "subset" => "\u{2282}",
        "subseteq" => "\u{2286}",
        "supset" => "\u{2283}",
        "supseteq" => "\u{2287}",
        "perp" => "\u{22A5}",
        "parallel" => "\u{2225}",
        "mid" => "|",
        "models" => "\u{22A8}",
        "vdash" => "\u{22A2}",
        _ => return None,
    })
}

/// Binary operators (`+`-like spacing).
fn binary(name: &str) -> Option<&'static str> {
    Some(match name {
        "times" => "\u{00D7}",
        "cdot" => "\u{22C5}",
        "div" => "\u{00F7}",
        "pm" => "\u{00B1}",
        "mp" => "\u{2213}",
        "cup" => "\u{222A}",
        "cap" => "\u{2229}",
        "setminus" => "\u{2216}",
        "land" | "wedge" => "\u{2227}",
        "lor" | "vee" => "\u{2228}",
        "oplus" => "\u{2295}",
        "otimes" => "\u{2297}",
        "circ" => "\u{2218}",
        "bullet" => "\u{2219}",
        "star" => "\u{22C6}",
        "ast" => "\u{2217}",
        _ => return None,
    })
}

/// Ordinary symbols and Greek letters: the glyph and whether it is a
/// variable (italic, like TeX's lowercase Greek).
fn symbol(name: &str) -> Option<(&'static str, bool)> {
    Some(match name {
        "alpha" => ("\u{03B1}", true),
        "beta" => ("\u{03B2}", true),
        "gamma" => ("\u{03B3}", true),
        "delta" => ("\u{03B4}", true),
        "epsilon" => ("\u{03F5}", true),
        "varepsilon" => ("\u{03B5}", true),
        "zeta" => ("\u{03B6}", true),
        "eta" => ("\u{03B7}", true),
        "theta" => ("\u{03B8}", true),
        "vartheta" => ("\u{03D1}", true),
        "iota" => ("\u{03B9}", true),
        "kappa" => ("\u{03BA}", true),
        "lambda" => ("\u{03BB}", true),
        "mu" => ("\u{03BC}", true),
        "nu" => ("\u{03BD}", true),
        "xi" => ("\u{03BE}", true),
        "pi" => ("\u{03C0}", true),
        "varpi" => ("\u{03D6}", true),
        "rho" => ("\u{03C1}", true),
        "varrho" => ("\u{03F1}", true),
        "sigma" => ("\u{03C3}", true),
        "varsigma" => ("\u{03C2}", true),
        "tau" => ("\u{03C4}", true),
        "upsilon" => ("\u{03C5}", true),
        "phi" => ("\u{03D5}", true),
        "varphi" => ("\u{03C6}", true),
        "chi" => ("\u{03C7}", true),
        "psi" => ("\u{03C8}", true),
        "omega" => ("\u{03C9}", true),
        "Gamma" => ("\u{0393}", false),
        "Delta" => ("\u{0394}", false),
        "Theta" => ("\u{0398}", false),
        "Lambda" => ("\u{039B}", false),
        "Xi" => ("\u{039E}", false),
        "Pi" => ("\u{03A0}", false),
        "Sigma" => ("\u{03A3}", false),
        "Upsilon" => ("\u{03A5}", false),
        "Phi" => ("\u{03A6}", false),
        "Psi" => ("\u{03A8}", false),
        "Omega" => ("\u{03A9}", false),
        "infty" => ("\u{221E}", false),
        "partial" => ("\u{2202}", false),
        "nabla" => ("\u{2207}", false),
        "sum" => ("\u{2211}", false),
        "prod" => ("\u{220F}", false),
        "int" => ("\u{222B}", false),
        "iint" => ("\u{222C}", false),
        "oint" => ("\u{222E}", false),
        "forall" => ("\u{2200}", false),
        "exists" => ("\u{2203}", false),
        "nexists" => ("\u{2204}", false),
        "emptyset" | "varnothing" => ("\u{2205}", false),
        "neg" | "lnot" => ("\u{00AC}", false),
        "ldots" | "dots" | "hdots" => ("\u{2026}", false),
        "cdots" => ("\u{22EF}", false),
        "vdots" => ("\u{22EE}", false),
        "ddots" => ("\u{22F1}", false),
        "prime" => ("\u{2032}", false),
        "degree" => ("\u{00B0}", false),
        "angle" => ("\u{2220}", false),
        "triangle" => ("\u{25B3}", false),
        "hbar" => ("\u{210F}", true),
        "ell" => ("\u{2113}", true),
        "Re" => ("\u{211C}", false),
        "Im" => ("\u{2111}", false),
        "aleph" => ("\u{2135}", false),
        "wp" => ("\u{2118}", true),
        "lceil" => ("\u{2308}", false),
        "rceil" => ("\u{2309}", false),
        "lfloor" => ("\u{230A}", false),
        "rfloor" => ("\u{230B}", false),
        "langle" => ("\u{27E8}", false),
        "rangle" => ("\u{27E9}", false),
        "lvert" | "rvert" | "vert" => ("|", false),
        "lVert" | "rVert" | "Vert" => ("\u{2016}", false),
        "checkmark" => ("\u{2713}", false),
        "top" => ("\u{22A4}", false),
        "bot" => ("\u{22A5}", false),
        "therefore" => ("\u{2234}", false),
        "because" => ("\u{2235}", false),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(src: &str) -> Vec<(String, MathStyle)> {
        typeset(src)
            .into_iter()
            .map(|r| (r.text, r.style))
            .collect()
    }

    #[test]
    fn big_o_of_n_squared() {
        assert_eq!(plain("O(n^2)"), "O(n²)");
        assert_eq!(
            styles("O(n^2)"),
            vec![
                ("O".to_string(), MathStyle::Italic),
                ("(".to_string(), MathStyle::Upright),
                ("n".to_string(), MathStyle::Italic),
                ("²)".to_string(), MathStyle::Upright),
            ]
        );
        assert_eq!(plain("O(n)"), "O(n)");
        assert_eq!(plain("O(1)"), "O(1)");
        assert_eq!(plain("O(n \\log n)"), "O(n\u{2009}log\u{2009}n)");
    }

    #[test]
    fn letters_italic_digits_and_functions_upright() {
        let runs = typeset("2x + \\sin(\\theta) = 1");
        let italic: String = runs
            .iter()
            .filter(|r| r.style == MathStyle::Italic)
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(italic, "xθ");
        assert_eq!(plain("2x + \\sin(\\theta) = 1"), "2x + sin(θ) = 1");
        assert_eq!(plain("\\sin x"), "sin\u{2009}x");
    }

    #[test]
    fn operators_space_between_operands_only() {
        assert_eq!(plain("n - 1"), "n − 1");
        assert_eq!(plain("n-1"), "n − 1");
        assert_eq!(plain("-1"), "−1");
        assert_eq!(plain("f(-x) = -f(x)"), "f(−x) = −f(x)");
        assert_eq!(plain("a \\times b \\cdot c"), "a × b ⋅ c");
        assert_eq!(plain("x \\leq y \\neq z \\to \\infty"), "x ≤ y ≠ z → ∞");
        assert_eq!(plain("f(x, y)"), "f(x, y)");
    }

    #[test]
    fn scripts_use_small_glyphs_or_fall_back() {
        assert_eq!(plain("x^{n-1}"), "xⁿ⁻¹");
        assert_eq!(plain("x_1 + x_2"), "x₁ + x₂");
        assert_eq!(plain("a_{ij}"), "aᵢⱼ");
        assert_eq!(plain("\\sum_{i=1}^{n} i"), "∑ᵢ₌₁ⁿ\u{2009}i");
        assert_eq!(plain("e^{i\\pi} + 1 = 0"), "e^(iπ) + 1 = 0");
        assert_eq!(plain("x^\\prime"), "x′");
        assert_eq!(plain("10^{10}"), "10¹⁰");
        assert_eq!(plain("x^{a^b}"), "x^(a^b)");
    }

    #[test]
    fn fractions_roots_and_text() {
        assert_eq!(plain("\\frac{1}{2}"), "¹⁄₂");
        assert_eq!(plain("\\frac{a}{b}"), "a/b");
        assert_eq!(plain("\\frac{n(n-1)}{2}"), "(n(n − 1))/2");
        assert_eq!(plain("\\frac{(a+b)}{2}"), "(a + b)/2");
        assert_eq!(plain("\\sqrt{2}"), "√2");
        assert_eq!(plain("\\sqrt{x^2 + y^2}"), "√(x² + y²)");
        assert_eq!(plain("\\sqrt[3]{x}"), "∛x");
        assert_eq!(plain("\\text{cost} = 5"), "cost = 5");
        assert_eq!(
            styles("\\text{if } x > 0"),
            vec![
                ("if ".to_string(), MathStyle::Upright),
                ("x".to_string(), MathStyle::Italic),
                (" > 0".to_string(), MathStyle::Upright),
            ]
        );
        assert_eq!(plain("\\mathbf{v} \\in \\mathbb{R}^n"), "v ∈ ℝⁿ");
        assert_eq!(
            typeset("\\mathbf{v}")[0],
            MathRun {
                text: "v".into(),
                style: MathStyle::Bold
            }
        );
        assert_eq!(plain("\\hat{x} \\bar{y}"), "x\u{302}y\u{304}");
        assert_eq!(plain("\\left( \\frac{a}{b} \\right)"), "(a/b)");
        assert_eq!(plain("\\alpha\\beta"), "αβ");
        assert_eq!(plain("\\Delta x"), "Δx");
    }

    #[test]
    fn unknown_commands_show_their_name() {
        assert_eq!(plain("\\foo x"), "foox");
        assert_eq!(plain(""), "");
        assert_eq!(plain("{"), "");
        assert_eq!(plain("\\frac{1}"), "1/");
    }
}
