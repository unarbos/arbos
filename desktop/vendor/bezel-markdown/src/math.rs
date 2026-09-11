//! TeX mathematics, read and painted.
//!
//! No engine and no font tables: a tokenizer, a recursive-descent parser over
//! the subset of LaTeX that prose about mathematics actually uses, and two ways
//! out of the tree. [`to_unicode`] flattens a formula to one string — `\mathbb
//! R` to `ℝ`, `x^2` to `𝑥²` — which is what an inline span holds, so a reader
//! can select and copy it like any other text. [`typeset`] lays a display
//! formula out in gpui boxes: fractions stack, scripts shrink and shift, big
//! operators carry their limits, delimiters grow with what they hold.
//!
//! Nothing here panics and nothing is dropped. A control word this does not
//! know paints as its own name in upright text; a brace that never closes runs
//! to the end of the formula; an environment with no `\end` ends where the
//! source does.

use gpui::{AnyElement, Div, Hsla, SharedString, div, prelude::*, px};

use self::Class::{Big, Binary, Ordinary, Relation};

/// The face mathematics is set in. Its Mathematical Alphanumeric Symbols are
/// what make `𝑥` italic and `ℝ` double-struck without a second font style —
/// STIX Two Math has no italic face to ask for. "STIX Two Text" is the family
/// beside it for prose; gpui falls back to the system face when neither is
/// installed.
pub const FONT: &str = "STIX Two Math";

/// Line box over glyph size. Tight, so a stack of boxes stays a stack.
const LINE: f32 = 1.2;
/// Scripts and limits, relative to their base.
const SCRIPT: f32 = 0.7;
/// A big operator's glyph, relative to the text around it.
const BIG: f32 = 1.4;
/// Air on either side of a relation and of a binary operator, in em.
const REL_GAP: f32 = 0.22;
const BIN_GAP: f32 = 0.16;
/// What a `\,` measures, in em; the other spacing commands are multiples.
const THIN: f32 = 0.167;

/// One formula, parsed.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Row(Vec<Node>),
    /// Letters, digits and symbols already mapped to the code points that
    /// paint them: `𝑥`, `12`, `ℝ`, `∞`, `(`.
    Atom(String),
    /// A relation or a binary operator — what gets air on either side.
    Op(String),
    /// Upright words: `\text{…}`, `\mathrm{…}`, a function name, a control
    /// word this does not know.
    Text(String),
    Frac {
        num: Box<Node>,
        den: Box<Node>,
        /// `\binom` is a fraction with no rule.
        bar: bool,
    },
    Scripts {
        base: Box<Node>,
        sup: Option<Box<Node>>,
        sub: Option<Box<Node>>,
    },
    Sqrt {
        index: Option<Box<Node>>,
        body: Box<Node>,
    },
    BigOp {
        op: String,
        sup: Option<Box<Node>>,
        sub: Option<Box<Node>>,
        /// Limits stacked over and under the operator (`∑`, `lim`) rather
        /// than set beside it (`∫`).
        limits: bool,
    },
    Fenced {
        left: String,
        body: Box<Node>,
        right: String,
    },
    /// A delimiter sized by hand — `\Big\{` — which TeX does not pair.
    Delim {
        glyph: String,
        scale: f32,
    },
    /// A mark over one letter, as the combining character that draws it.
    Accent {
        mark: char,
        body: Box<Node>,
    },
    Matrix {
        kind: MatrixKind,
        rows: Vec<Vec<Node>>,
    },
    /// `\overset` and `\stackrel`: something small over something.
    Over {
        top: Box<Node>,
        body: Box<Node>,
    },
    Under {
        bottom: Box<Node>,
        body: Box<Node>,
    },
    Overline(Box<Node>),
    Underline(Box<Node>),
    /// Horizontal space, in em.
    Space(f32),
    /// `\\` outside an environment.
    Break,
}

impl Node {
    fn boxed(self) -> Box<Node> {
        Box::new(self)
    }

    /// A row of one is the one.
    fn row(mut items: Vec<Node>) -> Node {
        match items.len() {
            1 => items.pop().unwrap_or(Node::Row(Vec::new())),
            _ => Node::Row(items),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixKind {
    /// `pmatrix`, `bmatrix`, `Bmatrix`, `vmatrix`, `Vmatrix`, `matrix`,
    /// `array`: every cell centred, fenced or not.
    Matrix,
    /// A brace on the left, rows flush left.
    Cases,
    /// `aligned`, `align*`, `gather`: columns alternate right and left, so the
    /// relation each row was split at lines up.
    Aligned,
}

// ─── tokens ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// `\alpha`, and the one-character controls `\{` `\,` `\\`.
    Control(String),
    /// The braces of a `\text{…}`, a `\begin{…}`: what is between them, whole.
    Raw(String),
    Open,
    Close,
    Sup,
    Sub,
    Amp,
    Newline,
    Number(String),
    Char(char),
}

/// Controls whose argument is words, not mathematics.
const TEXT_LIKE: [&str; 10] = [
    "text",
    "textrm",
    "textbf",
    "textit",
    "textsf",
    "texttt",
    "operatorname",
    "mathrm",
    "mbox",
    "hbox",
];

fn tokenize(tex: &str) -> Vec<Token> {
    let chars: Vec<char> = tex.chars().collect();
    let mut tokens = Vec::new();
    let mut ix = 0;
    while ix < chars.len() {
        let c = chars[ix];
        ix += 1;
        match c {
            '\\' => {
                let Some(&next) = chars.get(ix) else {
                    tokens.push(Token::Char('\\'));
                    break;
                };
                if next.is_ascii_alphabetic() {
                    let start = ix;
                    while ix < chars.len() && chars[ix].is_ascii_alphabetic() {
                        ix += 1;
                    }
                    // `\operatorname*`, `\align*`: the star is part of the name.
                    if chars.get(ix) == Some(&'*') {
                        ix += 1;
                    }
                    let name: String = chars[start..ix].iter().collect();
                    let raw = TEXT_LIKE.contains(&name.as_str())
                        || matches!(name.as_str(), "begin" | "end");
                    tokens.push(Token::Control(name));
                    if raw {
                        while ix < chars.len() && chars[ix].is_whitespace() {
                            ix += 1;
                        }
                        let (body, after) = raw_group(&chars, ix);
                        tokens.push(Token::Raw(body));
                        ix = after;
                    }
                } else {
                    ix += 1;
                    tokens.push(match next {
                        '\\' => Token::Newline,
                        other => Token::Control(other.to_string()),
                    });
                }
            }
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '^' => tokens.push(Token::Sup),
            '_' => tokens.push(Token::Sub),
            '&' => tokens.push(Token::Amp),
            c if c.is_whitespace() => {}
            c if c.is_ascii_digit() => {
                let start = ix - 1;
                while ix < chars.len()
                    && (chars[ix].is_ascii_digit()
                        || (chars[ix] == '.'
                            && chars.get(ix + 1).is_some_and(char::is_ascii_digit)))
                {
                    ix += 1;
                }
                tokens.push(Token::Number(chars[start..ix].iter().collect()));
            }
            // A comment runs to the end of its line.
            '%' => {
                while ix < chars.len() && chars[ix] != '\n' {
                    ix += 1;
                }
            }
            c => tokens.push(Token::Char(c)),
        }
    }
    tokens
}

/// The text of a brace group starting at `at`, braces balanced, and where the
/// scan stopped. Without a brace the group is the one character there.
fn raw_group(chars: &[char], at: usize) -> (String, usize) {
    if chars.get(at) != Some(&'{') {
        return match chars.get(at) {
            Some(c) => (c.to_string(), at + 1),
            None => (String::new(), at),
        };
    }
    let mut depth = 0usize;
    let mut ix = at;
    while ix < chars.len() {
        match chars[ix] {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return (chars[at + 1..ix].iter().collect(), ix + 1);
                }
            }
            _ => {}
        }
        ix += 1;
    }
    (chars[at + 1..].iter().collect(), chars.len())
}

// ─── symbols ─────────────────────────────────────────────────────────────────

/// How a symbol sits among its neighbours.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Ordinary,
    Binary,
    Relation,
    /// A big operator, and whether its limits stack.
    Big(bool),
}

/// Function names: upright, with a hair of space after them.
const FUNCTIONS: [&str; 29] = [
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh",
    "tanh", "coth", "log", "ln", "lg", "exp", "det", "dim", "ker", "deg", "gcd", "hom", "arg",
    "Pr", "tr", "rank", "sgn", "id",
];

/// The alphabet a run of letters is set in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Italic,
    Upright,
    Bold,
    BoldItalic,
    Blackboard,
    Script,
    Fraktur,
    Sans,
    Mono,
}

/// A control word that names a glyph, and how it behaves.
fn symbol(name: &str) -> Option<(&'static str, Class)> {
    let s = match name {
        // Greek.
        "alpha" => ("𝛼", Ordinary),
        "beta" => ("𝛽", Ordinary),
        "gamma" => ("𝛾", Ordinary),
        "delta" => ("𝛿", Ordinary),
        "epsilon" => ("𝜖", Ordinary),
        "varepsilon" => ("𝜀", Ordinary),
        "zeta" => ("𝜁", Ordinary),
        "eta" => ("𝜂", Ordinary),
        "theta" => ("𝜃", Ordinary),
        "vartheta" => ("𝜗", Ordinary),
        "iota" => ("𝜄", Ordinary),
        "kappa" => ("𝜅", Ordinary),
        "varkappa" => ("𝜘", Ordinary),
        "lambda" => ("𝜆", Ordinary),
        "mu" => ("𝜇", Ordinary),
        "nu" => ("𝜈", Ordinary),
        "xi" => ("𝜉", Ordinary),
        "omicron" => ("𝜊", Ordinary),
        "pi" => ("𝜋", Ordinary),
        "varpi" => ("𝜛", Ordinary),
        "rho" => ("𝜌", Ordinary),
        "varrho" => ("𝜚", Ordinary),
        "sigma" => ("𝜎", Ordinary),
        "varsigma" => ("𝜍", Ordinary),
        "tau" => ("𝜏", Ordinary),
        "upsilon" => ("𝜐", Ordinary),
        "phi" => ("𝜙", Ordinary),
        "varphi" => ("𝜑", Ordinary),
        "chi" => ("𝜒", Ordinary),
        "psi" => ("𝜓", Ordinary),
        "omega" => ("𝜔", Ordinary),
        "Gamma" => ("Γ", Ordinary),
        "Delta" => ("Δ", Ordinary),
        "Theta" => ("Θ", Ordinary),
        "Lambda" => ("Λ", Ordinary),
        "Xi" => ("Ξ", Ordinary),
        "Pi" => ("Π", Ordinary),
        "Sigma" => ("Σ", Ordinary),
        "Upsilon" => ("Υ", Ordinary),
        "Phi" => ("Φ", Ordinary),
        "Psi" => ("Ψ", Ordinary),
        "Omega" => ("Ω", Ordinary),
        // Arrows.
        "to" | "rightarrow" => ("→", Relation),
        "leftarrow" | "gets" => ("←", Relation),
        "leftrightarrow" => ("↔", Relation),
        "mapsto" => ("↦", Relation),
        "longmapsto" => ("⟼", Relation),
        "Rightarrow" | "implies" => ("⇒", Relation),
        "Leftarrow" | "impliedby" => ("⇐", Relation),
        "Leftrightarrow" | "iff" => ("⇔", Relation),
        "longrightarrow" => ("⟶", Relation),
        "longleftarrow" => ("⟵", Relation),
        "Longrightarrow" => ("⟹", Relation),
        "Longleftarrow" => ("⟸", Relation),
        "Longleftrightarrow" => ("⟺", Relation),
        "hookrightarrow" => ("↪", Relation),
        "hookleftarrow" => ("↩", Relation),
        "rightharpoonup" => ("⇀", Relation),
        "leftharpoonup" => ("↼", Relation),
        "rightleftharpoons" => ("⇌", Relation),
        "uparrow" => ("↑", Relation),
        "downarrow" => ("↓", Relation),
        "updownarrow" => ("↕", Relation),
        "Uparrow" => ("⇑", Relation),
        "Downarrow" => ("⇓", Relation),
        "nearrow" => ("↗", Relation),
        "searrow" => ("↘", Relation),
        "swarrow" => ("↙", Relation),
        "nwarrow" => ("↖", Relation),
        "twoheadrightarrow" => ("↠", Relation),
        "rightsquigarrow" | "leadsto" => ("⇝", Relation),
        // Relations.
        "leq" | "le" => ("≤", Relation),
        "geq" | "ge" => ("≥", Relation),
        "leqslant" => ("⩽", Relation),
        "geqslant" => ("⩾", Relation),
        "ll" => ("≪", Relation),
        "gg" => ("≫", Relation),
        "neq" | "ne" => ("≠", Relation),
        "approx" => ("≈", Relation),
        "equiv" => ("≡", Relation),
        "sim" => ("∼", Relation),
        "simeq" => ("≃", Relation),
        "cong" => ("≅", Relation),
        "propto" => ("∝", Relation),
        "doteq" => ("≐", Relation),
        "triangleq" => ("≜", Relation),
        "asymp" => ("≍", Relation),
        "prec" => ("≺", Relation),
        "succ" => ("≻", Relation),
        "preceq" => ("⪯", Relation),
        "succeq" => ("⪰", Relation),
        "in" => ("∈", Relation),
        "notin" => ("∉", Relation),
        "ni" | "owns" => ("∋", Relation),
        "subset" => ("⊂", Relation),
        "subseteq" => ("⊆", Relation),
        "subsetneq" => ("⊊", Relation),
        "supset" => ("⊃", Relation),
        "supseteq" => ("⊇", Relation),
        "supsetneq" => ("⊋", Relation),
        "sqsubseteq" => ("⊑", Relation),
        "sqsupseteq" => ("⊒", Relation),
        "perp" => ("⊥", Relation),
        "parallel" => ("∥", Relation),
        "nparallel" => ("∦", Relation),
        "mid" => ("∣", Relation),
        "nmid" => ("∤", Relation),
        "vdash" => ("⊢", Relation),
        "dashv" => ("⊣", Relation),
        "models" => ("⊨", Relation),
        "vDash" => ("⊨", Relation),
        "Vdash" => ("⊩", Relation),
        "bowtie" => ("⋈", Relation),
        "smile" => ("⌣", Relation),
        "frown" => ("⌢", Relation),
        "colon" => (":", Ordinary),
        // Binary operators.
        "pm" => ("±", Binary),
        "mp" => ("∓", Binary),
        "times" => ("×", Binary),
        "div" => ("÷", Binary),
        "cdot" => ("⋅", Binary),
        "ast" => ("∗", Binary),
        "star" => ("⋆", Binary),
        "circ" => ("∘", Binary),
        "bullet" => ("∙", Binary),
        "cup" => ("∪", Binary),
        "cap" => ("∩", Binary),
        "sqcup" => ("⊔", Binary),
        "sqcap" => ("⊓", Binary),
        "setminus" | "smallsetminus" => ("∖", Binary),
        "wedge" | "land" => ("∧", Binary),
        "vee" | "lor" => ("∨", Binary),
        "oplus" => ("⊕", Binary),
        "ominus" => ("⊖", Binary),
        "otimes" => ("⊗", Binary),
        "oslash" => ("⊘", Binary),
        "odot" => ("⊙", Binary),
        "boxplus" => ("⊞", Binary),
        "boxtimes" => ("⊠", Binary),
        "uplus" => ("⊎", Binary),
        "amalg" => ("⨿", Binary),
        "dagger" => ("†", Binary),
        "ddagger" => ("‡", Binary),
        "wr" => ("≀", Binary),
        "diamond" => ("⋄", Binary),
        "triangleleft" => ("◃", Binary),
        "triangleright" => ("▹", Binary),
        "bigtriangleup" => ("△", Binary),
        "bigtriangledown" => ("▽", Binary),
        // Ordinary symbols.
        "infty" => ("∞", Ordinary),
        "partial" => ("∂", Ordinary),
        "nabla" => ("∇", Ordinary),
        "hbar" => ("ℏ", Ordinary),
        "ell" => ("ℓ", Ordinary),
        "Re" => ("ℜ", Ordinary),
        "Im" => ("ℑ", Ordinary),
        "aleph" => ("ℵ", Ordinary),
        "beth" => ("ℶ", Ordinary),
        "wp" => ("℘", Ordinary),
        "prime" => ("′", Ordinary),
        "degree" => ("°", Ordinary),
        "emptyset" => ("∅", Ordinary),
        "varnothing" => ("∅", Ordinary),
        "forall" => ("∀", Ordinary),
        "exists" => ("∃", Ordinary),
        "nexists" => ("∄", Ordinary),
        "neg" | "lnot" => ("¬", Ordinary),
        "top" => ("⊤", Ordinary),
        "bot" => ("⊥", Ordinary),
        "angle" => ("∠", Ordinary),
        "measuredangle" => ("∡", Ordinary),
        "triangle" => ("△", Ordinary),
        "square" | "Box" => ("□", Ordinary),
        "blacksquare" => ("■", Ordinary),
        "clubsuit" => ("♣", Ordinary),
        "diamondsuit" => ("♢", Ordinary),
        "heartsuit" => ("♡", Ordinary),
        "spadesuit" => ("♠", Ordinary),
        "flat" => ("♭", Ordinary),
        "natural" => ("♮", Ordinary),
        "sharp" => ("♯", Ordinary),
        "checkmark" => ("✓", Ordinary),
        "imath" => ("𝚤", Ordinary),
        "jmath" => ("𝚥", Ordinary),
        "cdots" => ("⋯", Ordinary),
        "ldots" | "dots" | "dotsc" | "dotso" => ("…", Ordinary),
        "dotsb" | "dotsm" => ("⋯", Ordinary),
        "vdots" => ("⋮", Ordinary),
        "ddots" => ("⋱", Ordinary),
        "backslash" => ("\\", Ordinary),
        "%" => ("%", Ordinary),
        "#" => ("#", Ordinary),
        "&" => ("&", Ordinary),
        "_" => ("_", Ordinary),
        "$" => ("$", Ordinary),
        "{" | "lbrace" => ("{", Ordinary),
        "}" | "rbrace" => ("}", Ordinary),
        "|" | "Vert" | "lVert" | "rVert" => ("‖", Ordinary),
        "vert" | "lvert" | "rvert" => ("|", Ordinary),
        "langle" => ("⟨", Ordinary),
        "rangle" => ("⟩", Ordinary),
        "lfloor" => ("⌊", Ordinary),
        "rfloor" => ("⌋", Ordinary),
        "lceil" => ("⌈", Ordinary),
        "rceil" => ("⌉", Ordinary),
        "lbrack" => ("[", Ordinary),
        "rbrack" => ("]", Ordinary),
        "lparen" => ("(", Ordinary),
        "rparen" => (")", Ordinary),
        "llbracket" => ("⟦", Ordinary),
        "rrbracket" => ("⟧", Ordinary),
        // Big operators.
        "sum" => ("∑", Big(true)),
        "prod" => ("∏", Big(true)),
        "coprod" => ("∐", Big(true)),
        "bigcup" => ("⋃", Big(true)),
        "bigcap" => ("⋂", Big(true)),
        "bigsqcup" => ("⨆", Big(true)),
        "bigoplus" => ("⨁", Big(true)),
        "bigotimes" => ("⨂", Big(true)),
        "bigodot" => ("⨀", Big(true)),
        "bigvee" => ("⋁", Big(true)),
        "bigwedge" => ("⋀", Big(true)),
        "biguplus" => ("⨄", Big(true)),
        "int" => ("∫", Big(false)),
        "iint" => ("∬", Big(false)),
        "iiint" => ("∭", Big(false)),
        "oint" => ("∮", Big(false)),
        "oiint" => ("∯", Big(false)),
        "lim" => ("lim", Big(true)),
        "limsup" => ("lim sup", Big(true)),
        "liminf" => ("lim inf", Big(true)),
        "sup" => ("sup", Big(true)),
        "inf" => ("inf", Big(true)),
        "max" => ("max", Big(true)),
        "min" => ("min", Big(true)),
        "argmax" => ("arg max", Big(true)),
        "argmin" => ("arg min", Big(true)),
        _ => return None,
    };
    Some(s)
}

/// The combining mark an accent command draws.
fn accent(name: &str) -> Option<char> {
    Some(match name {
        "hat" | "widehat" => '\u{302}',
        "bar" => '\u{304}',
        "vec" => '\u{20D7}',
        "tilde" | "widetilde" => '\u{303}',
        "dot" => '\u{307}',
        "ddot" => '\u{308}',
        "dddot" => '\u{20DB}',
        "check" => '\u{30C}',
        "breve" => '\u{306}',
        "acute" => '\u{301}',
        "grave" => '\u{300}',
        "mathring" => '\u{30A}',
        _ => return None,
    })
}

/// How much a `\big` family command scales its delimiter — KaTeX's steps.
fn big_scale(name: &str) -> Option<f32> {
    let base = name
        .strip_suffix('l')
        .or_else(|| name.strip_suffix('r'))
        .or_else(|| name.strip_suffix('m'))
        .unwrap_or(name);
    Some(match base {
        "big" => 1.2,
        "Big" => 1.8,
        "bigg" => 2.4,
        "Bigg" => 3.0,
        _ => return None,
    })
}

fn font_style(name: &str) -> Option<Style> {
    Some(match name {
        "mathbb" | "Bbb" => Style::Blackboard,
        "mathcal" | "mathscr" => Style::Script,
        "mathfrak" => Style::Fraktur,
        "mathbf" | "bf" => Style::Bold,
        "boldsymbol" | "bm" | "pmb" => Style::BoldItalic,
        "mathit" | "it" => Style::Italic,
        "mathsf" => Style::Sans,
        "mathtt" | "tt" => Style::Mono,
        "mathnormal" | "rm" => Style::Upright,
        _ => return None,
    })
}

/// A letter or digit in an alphabet, by way of the Mathematical Alphanumeric
/// Symbols block and the Letterlike Symbols the block leaves holes for.
fn letter(c: char, style: Style) -> char {
    let (upper, lower, digit): (u32, u32, Option<u32>) = match style {
        Style::Upright => return c,
        Style::Italic => match c {
            'h' => return 'ℎ',
            _ => (0x1D434, 0x1D44E, None),
        },
        Style::Bold => (0x1D400, 0x1D41A, Some(0x1D7CE)),
        Style::BoldItalic => (0x1D468, 0x1D482, Some(0x1D7CE)),
        Style::Blackboard => match c {
            'C' => return 'ℂ',
            'H' => return 'ℍ',
            'N' => return 'ℕ',
            'P' => return 'ℙ',
            'Q' => return 'ℚ',
            'R' => return 'ℝ',
            'Z' => return 'ℤ',
            _ => (0x1D538, 0x1D552, Some(0x1D7D8)),
        },
        Style::Script => match c {
            'B' => return 'ℬ',
            'E' => return 'ℰ',
            'F' => return 'ℱ',
            'H' => return 'ℋ',
            'I' => return 'ℐ',
            'L' => return 'ℒ',
            'M' => return 'ℳ',
            'R' => return 'ℛ',
            'e' => return 'ℯ',
            'g' => return 'ℊ',
            'o' => return 'ℴ',
            _ => (0x1D49C, 0x1D4B6, None),
        },
        Style::Fraktur => match c {
            'C' => return 'ℭ',
            'H' => return 'ℌ',
            'I' => return 'ℑ',
            'R' => return 'ℜ',
            'Z' => return 'ℨ',
            _ => (0x1D504, 0x1D51E, None),
        },
        Style::Sans => (0x1D5A0, 0x1D5BA, Some(0x1D7E2)),
        Style::Mono => (0x1D670, 0x1D68A, Some(0x1D7F6)),
    };
    let mapped = match c {
        'A'..='Z' => Some(upper + (c as u32 - 'A' as u32)),
        'a'..='z' => Some(lower + (c as u32 - 'a' as u32)),
        '0'..='9' => digit.map(|base| base + (c as u32 - '0' as u32)),
        _ => None,
    };
    mapped.and_then(char::from_u32).unwrap_or(c)
}

/// The plain letter a mathematical italic code point stands for, so a script
/// can look it up in the superscript table.
fn unitalic(c: char) -> char {
    let code = c as u32;
    match code {
        0x1D434..=0x1D44D => char::from_u32('A' as u32 + code - 0x1D434).unwrap_or(c),
        0x1D44E..=0x1D467 => char::from_u32('a' as u32 + code - 0x1D44E).unwrap_or(c),
        _ if c == 'ℎ' => 'h',
        _ => c,
    }
}

fn delimiter_glyph(name: &str) -> Option<&'static str> {
    Some(match name {
        "(" | "lparen" => "(",
        ")" | "rparen" => ")",
        "[" | "lbrack" => "[",
        "]" | "rbrack" => "]",
        "{" | "lbrace" => "{",
        "}" | "rbrace" => "}",
        "|" | "vert" | "lvert" | "rvert" => "|",
        "Vert" | "lVert" | "rVert" => "‖",
        "langle" => "⟨",
        "rangle" => "⟩",
        "lfloor" => "⌊",
        "rfloor" => "⌋",
        "lceil" => "⌈",
        "rceil" => "⌉",
        "llbracket" => "⟦",
        "rrbracket" => "⟧",
        "uparrow" => "↑",
        "downarrow" => "↓",
        "/" => "/",
        "backslash" => "\\",
        "." => "",
        _ => return None,
    })
}

// ─── parser ──────────────────────────────────────────────────────────────────

/// What ends a row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// The end of the source.
    End,
    /// A closing brace.
    Close,
    /// `\right`.
    Right,
    /// `&`, `\\` or `\end` — a cell in an environment.
    Cell,
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
    style: Style,
}

/// Parse one formula. Never fails: what cannot be read is kept as text.
pub fn parse(tex: &str) -> Node {
    let mut parser = Parser {
        tokens: tokenize(tex),
        at: 0,
        style: Style::Italic,
    };
    let mut items = parser.row(Stop::End);
    // Whatever a malformed formula left unread — a stray `}` at the very end,
    // a `\right` with no `\left` — still has to come out.
    while parser.at < parser.tokens.len() {
        parser.at += 1;
        items.extend(parser.row(Stop::End));
    }
    Node::row(items)
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        if token.is_some() {
            self.at += 1;
        }
        token
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// Items until `stop`, which is left unread.
    fn row(&mut self, stop: Stop) -> Vec<Node> {
        let mut items = Vec::new();
        while let Some(token) = self.peek() {
            let ends = match token {
                Token::Close => stop == Stop::Close,
                Token::Amp | Token::Newline => stop == Stop::Cell,
                Token::Control(name) if name == "right" => stop == Stop::Right,
                Token::Control(name) if name == "end" => stop == Stop::Cell,
                _ => false,
            };
            if ends {
                break;
            }
            if let Some(item) = self.item() {
                items.push(item);
            }
        }
        items
    }

    /// One item with its scripts.
    fn item(&mut self) -> Option<Node> {
        let mut node = match self.peek()? {
            // A script with no base: `^2` is a superscript on nothing.
            Token::Sup | Token::Sub => Node::Row(Vec::new()),
            _ => self.nucleus()?,
        };
        let (mut sup, mut sub) = (None, None);
        loop {
            match self.peek() {
                Some(Token::Sup) => {
                    self.at += 1;
                    let arg = self.arg();
                    sup = Some(match sup {
                        Some(previous) => Node::Row(vec![previous, arg]),
                        None => arg,
                    });
                }
                Some(Token::Sub) => {
                    self.at += 1;
                    let arg = self.arg();
                    sub = Some(match sub {
                        Some(previous) => Node::Row(vec![previous, arg]),
                        None => arg,
                    });
                }
                Some(Token::Char('\'')) => {
                    let mut count = 0;
                    while self.eat(&Token::Char('\'')) {
                        count += 1;
                    }
                    let prime = Node::Atom(
                        match count {
                            1 => "′",
                            2 => "″",
                            3 => "‴",
                            _ => "⁗",
                        }
                        .to_string(),
                    );
                    sup = Some(match sup {
                        Some(previous) => Node::Row(vec![prime, previous]),
                        None => prime,
                    });
                }
                _ => break,
            }
        }
        if sup.is_none() && sub.is_none() {
            return Some(node);
        }
        if let Node::BigOp {
            sup: op_sup,
            sub: op_sub,
            ..
        } = &mut node
        {
            *op_sup = sup.map(Node::boxed);
            *op_sub = sub.map(Node::boxed);
            return Some(node);
        }
        Some(Node::Scripts {
            base: node.boxed(),
            sup: sup.map(Node::boxed),
            sub: sub.map(Node::boxed),
        })
    }

    /// The next argument: a group, or one item without scripts.
    fn arg(&mut self) -> Node {
        match self.peek() {
            Some(Token::Open) => {
                self.at += 1;
                let items = self.row(Stop::Close);
                self.eat(&Token::Close);
                Node::row(items)
            }
            Some(_) => self.nucleus().unwrap_or(Node::Row(Vec::new())),
            None => Node::Row(Vec::new()),
        }
    }

    fn styled_arg(&mut self, style: Style) -> Node {
        let saved = self.style;
        self.style = style;
        let node = self.arg();
        self.style = saved;
        node
    }

    /// An optional `[…]` argument.
    fn optional(&mut self) -> Option<Node> {
        if self.peek() != Some(&Token::Char('[')) {
            return None;
        }
        let saved = self.at;
        self.at += 1;
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None => {
                    self.at = saved;
                    return None;
                }
                Some(Token::Char(']')) => {
                    self.at += 1;
                    return Some(Node::row(items));
                }
                _ => {
                    if let Some(item) = self.item() {
                        items.push(item);
                    }
                }
            }
        }
    }

    /// One item, scripts not included.
    fn nucleus(&mut self) -> Option<Node> {
        let token = self.next()?;
        Some(match token {
            Token::Open => {
                let items = self.row(Stop::Close);
                self.eat(&Token::Close);
                Node::row(items)
            }
            // A brace nobody opened, a cell marker outside an environment, a
            // script marker with nothing to bind to: skipped.
            Token::Close | Token::Sup | Token::Sub => return None,
            Token::Amp => Node::Space(0.5),
            Token::Newline => Node::Break,
            Token::Raw(text) => Node::Text(text),
            Token::Number(digits) => Node::Atom(
                digits
                    .chars()
                    .map(|c| letter(c, self.style))
                    .collect(),
            ),
            Token::Char(c) => self.char(c),
            Token::Control(name) => self.control(&name),
        })
    }

    fn char(&self, c: char) -> Node {
        match c {
            'a'..='z' | 'A'..='Z' => Node::Atom(letter(c, self.style).to_string()),
            '+' => Node::Op("+".into()),
            '-' => Node::Op("−".into()),
            '*' => Node::Op("∗".into()),
            '=' => Node::Op("=".into()),
            '<' => Node::Op("<".into()),
            '>' => Node::Op(">".into()),
            ',' | ';' => Node::Row(vec![Node::Atom(c.to_string()), Node::Space(THIN)]),
            ':' => Node::Op(":".into()),
            '~' => Node::Space(0.33),
            _ => Node::Atom(c.to_string()),
        }
    }

    fn control(&mut self, name: &str) -> Node {
        if let Some((glyph, class)) = symbol(name) {
            return match class {
                Ordinary => Node::Atom(glyph.to_string()),
                Binary | Relation => Node::Op(glyph.to_string()),
                Big(limits) => Node::BigOp {
                    op: glyph.to_string(),
                    sup: None,
                    sub: None,
                    limits,
                },
            };
        }
        if FUNCTIONS.contains(&name) {
            return Node::Row(vec![Node::Text(name.to_string()), Node::Space(THIN)]);
        }
        if let Some(style) = font_style(name) {
            return self.styled_arg(style);
        }
        if let Some(mark) = accent(name) {
            return Node::Accent {
                mark,
                body: self.arg().boxed(),
            };
        }
        if let Some(scale) = big_scale(name) {
            return match self.delimiter() {
                Some(glyph) if !glyph.is_empty() => Node::Delim { glyph, scale },
                Some(_) => Node::Space(0.0),
                None => Node::Text(name.to_string()),
            };
        }
        match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" => Node::Frac {
                num: self.arg().boxed(),
                den: self.arg().boxed(),
                bar: true,
            },
            "binom" | "dbinom" | "tbinom" | "choose" => Node::Fenced {
                left: "(".into(),
                body: Node::Frac {
                    num: self.arg().boxed(),
                    den: self.arg().boxed(),
                    bar: false,
                }
                .boxed(),
                right: ")".into(),
            },
            "sqrt" => {
                let index = self.optional();
                Node::Sqrt {
                    index: index.map(Node::boxed),
                    body: self.arg().boxed(),
                }
            }
            "left" => {
                let left = self.delimiter().unwrap_or_default();
                let body = Node::row(self.row(Stop::Right));
                let right = match self.peek() {
                    Some(Token::Control(name)) if name == "right" => {
                        self.at += 1;
                        self.delimiter().unwrap_or_default()
                    }
                    _ => String::new(),
                };
                Node::Fenced {
                    left,
                    body: body.boxed(),
                    right,
                }
            }
            // A `\right` with no `\left` open: its delimiter paints plain.
            "right" => match self.delimiter() {
                Some(glyph) => Node::Atom(glyph),
                None => Node::Row(Vec::new()),
            },
            "begin" => self.environment(),
            // An `\end` nobody began — skip its name.
            "end" => {
                if matches!(self.peek(), Some(Token::Raw(_))) {
                    self.at += 1;
                }
                Node::Row(Vec::new())
            }
            "overline" | "overbrace" | "widebar" => Node::Overline(self.arg().boxed()),
            "underline" | "underbrace" => Node::Underline(self.arg().boxed()),
            "overset" | "stackrel" => {
                let top = self.arg();
                Node::Over {
                    top: top.boxed(),
                    body: self.arg().boxed(),
                }
            }
            "underset" => {
                let bottom = self.arg();
                Node::Under {
                    bottom: bottom.boxed(),
                    body: self.arg().boxed(),
                }
            }
            "xrightarrow" | "xleftarrow" => {
                let top = self.arg();
                let arrow = if name == "xrightarrow" { "⟶" } else { "⟵" };
                Node::Over {
                    top: top.boxed(),
                    body: Node::Op(arrow.into()).boxed(),
                }
            }
            "not" => {
                let inner = self.arg();
                negate(inner)
            }
            // Spacing.
            "," | "thinspace" => Node::Space(THIN),
            ":" | "medspace" | ">" => Node::Space(0.222),
            ";" | "thickspace" => Node::Space(0.278),
            "!" | "negthinspace" => Node::Space(0.0),
            " " | "space" | "nobreakspace" => Node::Space(0.33),
            "enspace" => Node::Space(0.5),
            "quad" => Node::Space(1.0),
            "qquad" => Node::Space(2.0),
            "hspace" | "hspace*" | "hskip" | "mspace" | "kern" | "mkern" => {
                self.arg();
                Node::Space(0.5)
            }
            "phantom" | "hphantom" | "vphantom" | "label" | "tag" | "tag*" | "ref"
            | "eqref" => {
                self.arg();
                Node::Row(Vec::new())
            }
            "displaystyle" | "textstyle" | "scriptstyle" | "scriptscriptstyle" | "limits"
            | "nolimits" | "nonumber" | "notag" | "left." | "right." | "allowbreak"
            | "relax" | "mathstrut" | "strut" => Node::Row(Vec::new()),
            // Words the tokenizer read whole.
            _ if TEXT_LIKE.contains(&name) => match self.next() {
                Some(Token::Raw(words)) if name == "mathrm" => Node::Atom(words),
                Some(Token::Raw(words)) if name == "operatorname" => {
                    Node::Row(vec![Node::Text(words), Node::Space(THIN)])
                }
                Some(Token::Raw(words)) => Node::Text(words),
                _ => Node::Row(Vec::new()),
            },
            // Unknown: its name, upright, so nothing an author wrote is lost.
            _ => Node::Text(name.to_string()),
        }
    }

    /// The delimiter after `\left`, `\right` or a `\big`: a character or a
    /// control word that names one. `None` when the next token is neither.
    fn delimiter(&mut self) -> Option<String> {
        let glyph = match self.peek()? {
            Token::Char(c) => delimiter_glyph(&c.to_string()),
            Token::Control(name) => delimiter_glyph(name),
            _ => None,
        }?;
        self.at += 1;
        Some(glyph.to_string())
    }

    /// `\begin{name} … \end{name}`, cells split at `&` and rows at `\\`.
    fn environment(&mut self) -> Node {
        let name = match self.next() {
            Some(Token::Raw(name)) => name,
            _ => String::new(),
        };
        // `array` carries a column spec before its body.
        if name.starts_with("array") && self.peek() == Some(&Token::Open) {
            self.arg();
        }
        let (kind, left, right) = match name.as_str() {
            "pmatrix" | "pmatrix*" => (MatrixKind::Matrix, "(", ")"),
            "bmatrix" | "bmatrix*" => (MatrixKind::Matrix, "[", "]"),
            "Bmatrix" | "Bmatrix*" => (MatrixKind::Matrix, "{", "}"),
            "vmatrix" | "vmatrix*" => (MatrixKind::Matrix, "|", "|"),
            "Vmatrix" | "Vmatrix*" => (MatrixKind::Matrix, "‖", "‖"),
            "cases" | "dcases" | "rcases" => (MatrixKind::Cases, "{", ""),
            "matrix" | "matrix*" | "array" | "smallmatrix" => (MatrixKind::Matrix, "", ""),
            _ => (MatrixKind::Aligned, "", ""),
        };

        let mut rows: Vec<Vec<Node>> = vec![Vec::new()];
        loop {
            let cell = Node::row(self.row(Stop::Cell));
            if let Some(row) = rows.last_mut() {
                row.push(cell);
            }
            match self.next() {
                Some(Token::Amp) => {}
                Some(Token::Newline) => {
                    // `\\[2pt]`: a row gap this does not measure.
                    let _ = self.optional();
                    rows.push(Vec::new());
                }
                Some(Token::Control(_)) => {
                    // `\end` and its name.
                    if matches!(self.peek(), Some(Token::Raw(_))) {
                        self.at += 1;
                    }
                    break;
                }
                _ => break,
            }
        }
        // A trailing `\\` leaves an empty last row behind.
        if rows
            .last()
            .is_some_and(|row| row.iter().all(|cell| *cell == Node::Row(Vec::new())))
            && rows.len() > 1
        {
            rows.pop();
        }
        let matrix = Node::Matrix { kind, rows };
        if left.is_empty() && right.is_empty() {
            matrix
        } else {
            Node::Fenced {
                left: left.into(),
                body: matrix.boxed(),
                right: right.into(),
            }
        }
    }
}

/// `\not` over a relation: the negated glyph where Unicode has one, a
/// combining stroke otherwise.
fn negate(node: Node) -> Node {
    let (glyph, is_op) = match &node {
        Node::Op(s) => (s.as_str(), true),
        Node::Atom(s) => (s.as_str(), false),
        _ => return node,
    };
    let struck = match glyph {
        "=" => "≠",
        "∈" => "∉",
        "∋" => "∌",
        "⊂" => "⊄",
        "⊃" => "⊅",
        "⊆" => "⊈",
        "⊇" => "⊉",
        "≤" => "≰",
        "≥" => "≱",
        "<" => "≮",
        ">" => "≯",
        "≡" => "≢",
        "∼" => "≁",
        "≈" => "≉",
        "≅" => "≇",
        "∣" => "∤",
        "∥" => "∦",
        "→" => "↛",
        "⇒" => "⇏",
        _ => {
            let mut s = glyph.to_string();
            s.push('\u{338}');
            return if is_op { Node::Op(s) } else { Node::Atom(s) };
        }
    };
    if is_op {
        Node::Op(struck.into())
    } else {
        Node::Atom(struck.into())
    }
}

// ─── unicode ─────────────────────────────────────────────────────────────────

/// A formula as one line of Unicode — what an inline span shows and copies.
pub fn to_unicode(tex: &str) -> String {
    let mut out = String::new();
    flatten(&parse(tex), &mut out, false);
    // Collapse the spaces that spacing commands and operators leave doubled.
    let mut collapsed = String::with_capacity(out.len());
    let mut last_space = true;
    for c in out.chars() {
        let space = c == ' ';
        if space && last_space {
            continue;
        }
        collapsed.push(c);
        last_space = space;
    }
    collapsed.trim().to_string()
}

/// Write `node` to `out`. `tight` drops the air around operators — inside a
/// script or a fraction there is no room for it.
fn flatten(node: &Node, out: &mut String, tight: bool) {
    match node {
        Node::Row(items) => {
            for (ix, item) in items.iter().enumerate() {
                match item {
                    Node::Op(op) if !tight => {
                        let unary = op == "−" && is_unary_position(items, ix);
                        if unary {
                            out.push_str(op);
                        } else {
                            out.push(' ');
                            out.push_str(op);
                            out.push(' ');
                        }
                    }
                    _ => flatten(item, out, tight),
                }
            }
        }
        Node::Atom(s) | Node::Op(s) | Node::Text(s) => out.push_str(s),
        Node::Frac { num, den, bar } => {
            let (n, d) = (flat_string(num, true), flat_string(den, true));
            if *bar {
                out.push_str(&parenthesize(num, &n));
                out.push('/');
                out.push_str(&parenthesize(den, &d));
            } else {
                out.push_str(&format!("({n} {d})"));
            }
        }
        Node::Scripts { base, sup, sub } => {
            flatten(base, out, tight);
            scripts(sub.as_deref(), sup.as_deref(), out);
        }
        Node::Sqrt { index, body } => {
            let radical = match index.as_deref().map(|ix| flat_string(ix, true)) {
                None => "√".to_string(),
                Some(ix) if ix == "3" => "∛".to_string(),
                Some(ix) if ix == "4" => "∜".to_string(),
                Some(ix) => format!("{}√", script_text(&ix, true)),
            };
            out.push_str(&radical);
            let inner = flat_string(body, false);
            out.push_str(&parenthesize(body, &inner));
        }
        Node::BigOp { op, sup, sub, .. } => {
            out.push_str(op);
            scripts(sub.as_deref(), sup.as_deref(), out);
            out.push(' ');
        }
        Node::Fenced { left, body, right } => {
            out.push_str(left);
            flatten(body, out, tight);
            out.push_str(right);
        }
        Node::Delim { glyph, .. } => out.push_str(glyph),
        // The mark goes on a plain letter: the math face attaches a combining
        // mark to `x` and not to `𝑥`, and a hat that does not show is worse
        // than a letter set upright.
        Node::Accent { mark, body } => {
            let inner = flat_string(body, true);
            let mut letters = inner.chars();
            match (letters.next(), letters.next()) {
                (Some(only), None) => out.push(unitalic(only)),
                _ => out.push_str(&inner),
            }
            out.push(*mark);
        }
        Node::Matrix { rows, .. } => {
            for (r, row) in rows.iter().enumerate() {
                if r > 0 {
                    out.push_str("; ");
                }
                for (c, cell) in row.iter().enumerate() {
                    if c > 0 {
                        out.push_str(", ");
                    }
                    flatten(cell, out, true);
                }
            }
        }
        Node::Over { top, body } => {
            flatten(body, out, tight);
            let above = flat_string(top, true);
            out.push_str(&format!("^({above})"));
        }
        Node::Under { bottom, body } => {
            flatten(body, out, tight);
            let below = flat_string(bottom, true);
            out.push_str(&format!("_({below})"));
        }
        Node::Overline(body) => {
            let inner = flat_string(body, true);
            for c in inner.chars() {
                out.push(c);
                out.push('\u{305}');
            }
        }
        Node::Underline(body) => {
            let inner = flat_string(body, true);
            for c in inner.chars() {
                out.push(c);
                out.push('\u{332}');
            }
        }
        Node::Space(em) => {
            if *em >= 0.5 {
                out.push(' ');
            } else if *em > 0.0 {
                out.push('\u{2009}');
            }
        }
        Node::Break => out.push(' '),
    }
}

/// A minus with nothing to its left is a sign, not a subtraction.
fn is_unary_position(items: &[Node], ix: usize) -> bool {
    match ix.checked_sub(1).map(|prev| &items[prev]) {
        None => true,
        Some(Node::Op(_)) | Some(Node::Space(_)) => true,
        Some(Node::Atom(s)) => matches!(s.as_str(), "(" | "[" | "{" | "⟨"),
        _ => false,
    }
}

fn flat_string(node: &Node, tight: bool) -> String {
    let mut out = String::new();
    flatten(node, &mut out, tight);
    out
}

/// `s` in parentheses when `node` is more than one thing — `(x+1)/2` rather
/// than `x+1/2`, but `12/5` stays bare.
fn parenthesize(node: &Node, s: &str) -> String {
    let compound = match node {
        Node::Atom(_) | Node::Text(_) => false,
        Node::Row(items) if items.len() == 1 => return parenthesize(&items[0], s),
        Node::Scripts { base, .. } => matches!(**base, Node::Row(_)),
        Node::Fenced { .. } => false,
        _ => s.chars().filter(|c| *c != '\u{2009}').count() > 1,
    };
    if compound {
        format!("({s})")
    } else {
        s.to_string()
    }
}

fn scripts(sub: Option<&Node>, sup: Option<&Node>, out: &mut String) {
    if let Some(sub) = sub {
        let s = flat_string(sub, true);
        out.push_str(&script_text(&s, false));
    }
    if let Some(sup) = sup {
        let s = flat_string(sup, true);
        out.push_str(&script_text(&s, true));
    }
}

/// A script as super- or subscript characters when every character has one;
/// otherwise `^x`, or `^(…)` for more than one character.
fn script_text(s: &str, sup: bool) -> String {
    let mapped: Option<String> = s
        .chars()
        .filter(|c| *c != '\u{2009}' && *c != ' ')
        .map(|c| script_char(c, sup))
        .collect();
    if let Some(mapped) = mapped
        && !mapped.is_empty()
    {
        return mapped;
    }
    let marker = if sup { '^' } else { '_' };
    let bare = s.trim();
    if bare.chars().count() == 1 {
        format!("{marker}{bare}")
    } else {
        format!("{marker}({bare})")
    }
}

fn script_char(c: char, sup: bool) -> Option<char> {
    let c = unitalic(c);
    let mapped = if sup {
        match c {
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            '+' => '⁺',
            '-' | '−' => '⁻',
            '=' => '⁼',
            '(' => '⁽',
            ')' => '⁾',
            'i' => 'ⁱ',
            'n' => 'ⁿ',
            '′' => '′',
            '″' => '″',
            '‴' => '‴',
            '*' | '∗' => '*',
            _ => return None,
        }
    } else {
        match c {
            '0' => '₀',
            '1' => '₁',
            '2' => '₂',
            '3' => '₃',
            '4' => '₄',
            '5' => '₅',
            '6' => '₆',
            '7' => '₇',
            '8' => '₈',
            '9' => '₉',
            '+' => '₊',
            '-' | '−' => '₋',
            '=' => '₌',
            '(' => '₍',
            ')' => '₎',
            'a' => 'ₐ',
            'e' => 'ₑ',
            'h' => 'ₕ',
            'i' => 'ᵢ',
            'j' => 'ⱼ',
            'k' => 'ₖ',
            'l' => 'ₗ',
            'm' => 'ₘ',
            'n' => 'ₙ',
            'o' => 'ₒ',
            'p' => 'ₚ',
            'r' => 'ᵣ',
            's' => 'ₛ',
            't' => 'ₜ',
            'u' => 'ᵤ',
            'v' => 'ᵥ',
            'x' => 'ₓ',
            _ => return None,
        }
    };
    Some(mapped)
}

// ─── layout ──────────────────────────────────────────────────────────────────

/// Lay a display formula out in gpui boxes, `size` pixels to the em.
///
/// Every row centres its items on one axis, and every compound box is built
/// so that axis runs through it where TeX's would — a fraction's rule, a big
/// operator's middle — which is what lets `items_center` stand in for a
/// baseline the layout engine does not report.
pub fn typeset(tex: &str, size: f32, color: Hsla) -> AnyElement {
    let node = parse(tex);
    div()
        .flex()
        .flex_col()
        .items_center()
        .font_family(FONT)
        .text_color(color)
        .whitespace_nowrap()
        .child(element(&node, size, color))
        .into_any_element()
}

fn element(node: &Node, size: f32, color: Hsla) -> AnyElement {
    match node {
        Node::Row(items) => row(items, size, color),
        Node::Atom(s) | Node::Op(s) => glyphs(s, size).into_any_element(),
        Node::Text(s) => glyphs(s, size).into_any_element(),
        Node::Frac { num, den, bar } => frac(num, den, *bar, size, color),
        Node::Scripts { base, sup, sub } => {
            let base_el = element(base, size, color);
            let reach = height_em(base) / LINE;
            scripts_beside(base_el, sup.as_deref(), sub.as_deref(), size, reach, color)
        }
        Node::Sqrt { index, body } => sqrt(index.as_deref(), body, size, color),
        Node::BigOp {
            op,
            sup,
            sub,
            limits,
        } => big_op(op, sup.as_deref(), sub.as_deref(), *limits, size, color),
        Node::Fenced { left, body, right } => {
            // A paren's ink is shorter than its em, so it grows a little past
            // the body's height to reach the fraction it holds.
            let scale = (height_em(body) / LINE * 1.15).max(1.0);
            div()
                .flex()
                .flex_row()
                .items_center()
                .children((!left.is_empty()).then(|| delimiter(left, size, scale)))
                .child(element(body, size, color))
                .children((!right.is_empty()).then(|| delimiter(right, size, scale)))
                .into_any_element()
        }
        Node::Delim { glyph, scale } => delimiter(glyph, size, *scale).into_any_element(),
        // Stacked rather than combined: the math face does not place a
        // combining mark over its italic alphabet, so the mark's spacing
        // glyph is set over the body by hand.
        Node::Accent { mark, body } => {
            // The spacing glyph that draws the mark, its size, and how far
            // above the body's box it sits — each glyph rides at its own
            // height in the face.
            let (glyph, scale, lift) = match mark {
                '\u{302}' => ("ˆ", 1.0, 0.42),
                '\u{304}' | '\u{305}' => ("¯", 1.0, 0.22),
                '\u{20D7}' => ("→", 0.6, 0.3),
                '\u{303}' => ("˜", 1.0, 0.42),
                '\u{307}' => ("˙", 1.0, 0.42),
                '\u{308}' => ("¨", 1.0, 0.42),
                '\u{20DB}' => ("⃛", 1.0, 0.42),
                '\u{30C}' => ("ˇ", 1.0, 0.42),
                '\u{306}' => ("˘", 1.0, 0.42),
                '\u{301}' => ("´", 1.0, 0.42),
                '\u{300}' => ("`", 1.0, 0.42),
                '\u{30A}' => ("˚", 1.0, 0.42),
                _ => ("ˆ", 1.0, 0.42),
            };
            // The mark floats over the body's box rather than stacking on it,
            // so the body keeps its place on the row's axis.
            div()
                .relative()
                .child(element(body, size, color))
                .child(
                    div()
                        .absolute()
                        .top(px(-lift * size))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .text_size(px(size * scale))
                        .line_height(px(size * scale))
                        .child(SharedString::from(glyph)),
                )
                .into_any_element()
        }
        Node::Matrix { kind, rows } => matrix(*kind, rows, size, color),
        Node::Over { top, body } => stack_over(
            element(top, size * SCRIPT, color),
            element(body, size, color),
            size,
        ),
        Node::Under { bottom, body } => div()
            .flex()
            .flex_col()
            .items_center()
            .child(element(body, size, color))
            .child(
                div()
                    .mt(px(-0.15 * size))
                    .child(element(bottom, size * SCRIPT, color)),
            )
            .into_any_element(),
        Node::Overline(body) => div()
            .border_t_1()
            .border_color(color)
            .pt(px(0.08 * size))
            .child(element(body, size, color))
            .into_any_element(),
        Node::Underline(body) => div()
            .border_b_1()
            .border_color(color)
            .pb(px(0.05 * size))
            .child(element(body, size, color))
            .into_any_element(),
        Node::Space(em) => div().flex_none().w(px(em.max(0.0) * size)).into_any_element(),
        Node::Break => div().into_any_element(),
    }
}

/// A run of glyphs in a line box of its own size.
fn glyphs(s: &str, size: f32) -> Div {
    div()
        .flex_none()
        .text_size(px(size))
        .line_height(px(size * LINE))
        .child(SharedString::from(s.to_string()))
}

/// A delimiter grown by `scale`. Its line box stays the glyph's own height so
/// a tall paren does not push the row apart; centring does the rest.
fn delimiter(glyph: &str, size: f32, scale: f32) -> Div {
    div()
        .flex_none()
        .text_size(px(size * scale))
        .line_height(px(size * scale))
        .child(SharedString::from(glyph.to_string()))
}

/// Items on one axis. `\\` splits the row into lines stacked and centred.
fn row(items: &[Node], size: f32, color: Hsla) -> AnyElement {
    let lines: Vec<&[Node]> = items.split(|item| *item == Node::Break).collect();
    if lines.len() > 1 {
        return div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(0.3 * size))
            .children(lines.into_iter().map(|line| row(line, size, color)))
            .into_any_element();
    }
    let mut out = div().flex().flex_row().items_center();
    for (ix, item) in items.iter().enumerate() {
        out = out.child(match item {
            Node::Op(op) => {
                let relation = is_relation(op);
                let unary = !relation && is_unary_position(items, ix);
                let gap = if unary {
                    0.0
                } else if relation {
                    REL_GAP
                } else {
                    BIN_GAP
                };
                glyphs(op, size).mx(px(gap * size)).into_any_element()
            }
            other => element(other, size, color),
        });
    }
    out.into_any_element()
}

fn is_relation(op: &str) -> bool {
    !matches!(
        op,
        "+" | "−"
            | "∗"
            | "±"
            | "∓"
            | "×"
            | "÷"
            | "⋅"
            | "⋆"
            | "∘"
            | "∙"
            | "∪"
            | "∩"
            | "⊔"
            | "⊓"
            | "∖"
            | "∧"
            | "∨"
            | "⊕"
            | "⊖"
            | "⊗"
            | "⊘"
            | "⊙"
            | "⊞"
            | "⊠"
            | "⊎"
            | "⨿"
            | "†"
            | "‡"
            | "≀"
            | "⋄"
            | "◃"
            | "▹"
            | "△"
            | "▽"
    )
}

fn frac(num: &Node, den: &Node, bar: bool, size: f32, color: Hsla) -> AnyElement {
    let rule = if bar {
        div().h(px(1.0)).w_full().bg(color).my(px(0.1 * size))
    } else {
        div().h(px(0.15 * size))
    };
    div()
        .flex()
        .flex_col()
        .items_center()
        .mx(px(0.12 * size))
        .child(div().px(px(0.1 * size)).child(element(num, size, color)))
        .child(rule)
        .child(div().px(px(0.1 * size)).child(element(den, size, color)))
        .into_any_element()
}

/// A base with its scripts in a column beside it. The column is taller than
/// the base by design and pinned to its centre, which puts the superscript's
/// baseline above the base's and the subscript's below it. `reach` is how
/// many line boxes tall the base is — an integral's glyph, a fenced fraction —
/// so the scripts climb to its corners.
fn scripts_beside(
    base: AnyElement,
    sup: Option<&Node>,
    sub: Option<&Node>,
    size: f32,
    reach: f32,
    color: Hsla,
) -> AnyElement {
    let small = size * SCRIPT;
    let slot = |node: Option<&Node>| match node {
        Some(node) => element(node, small, color),
        None => div().into_any_element(),
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .child(base)
        .child(
            div()
                .flex()
                .flex_col()
                .justify_between()
                .min_h(px(1.5 * size * reach.max(1.0)))
                .ml(px(0.04 * size))
                .child(slot(sup))
                .child(slot(sub)),
        )
        .into_any_element()
}

fn big_op(
    op: &str,
    sup: Option<&Node>,
    sub: Option<&Node>,
    limits: bool,
    size: f32,
    color: Hsla,
) -> AnyElement {
    let wordy = op.chars().all(|c| c.is_ascii_alphabetic() || c == ' ');
    // An integral's glyph is drawn slighter than a sum's, so it is set larger
    // to weigh the same.
    let scale = if wordy {
        1.0
    } else if limits {
        BIG
    } else {
        BIG * 1.2
    };
    let glyph = div()
        .flex_none()
        .text_size(px(size * scale))
        .line_height(px(size * scale * 1.1))
        .child(SharedString::from(op.to_string()));
    if !limits {
        return scripts_beside(
            glyph.into_any_element(),
            sup,
            sub,
            size,
            scale,
            color,
        )
        .into_any_element();
    }
    let small = size * SCRIPT;
    div()
        .flex()
        .flex_col()
        .items_center()
        .mx(px(0.15 * size))
        .children(sup.map(|sup| div().mb(px(-0.1 * size)).child(element(sup, small, color))))
        .child(glyph)
        .children(sub.map(|sub| div().mt(px(-0.1 * size)).child(element(sub, small, color))))
        .into_any_element()
}

fn sqrt(index: Option<&Node>, body: &Node, size: f32, color: Hsla) -> AnyElement {
    let body_em = height_em(body);
    // The radical's glyph reaches the overline: its size is the body's height
    // plus the line box the glyph does not fill.
    let radical = size * (body_em / LINE).max(1.0) * 1.15;
    let sign = div()
        .flex_none()
        .text_size(px(radical))
        .line_height(px(radical))
        .child(SharedString::from("√"));
    let sign = match index {
        Some(index) => div()
            .flex()
            .flex_row()
            .items_start()
            .child(
                div()
                    .mr(px(-0.35 * size))
                    .mt(px(-0.2 * size))
                    .child(element(index, size * 0.6, color)),
            )
            .child(sign)
            .into_any_element(),
        None => sign.into_any_element(),
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .child(sign)
        .child(
            div()
                .border_t_1()
                .border_color(color)
                .ml(px(-0.04 * size))
                .mt(px(-0.05 * radical))
                .pt(px(0.1 * size))
                .pr(px(0.08 * size))
                .child(element(body, size, color)),
        )
        .into_any_element()
}

/// `top` over `body`, tight.
fn stack_over(top: AnyElement, body: AnyElement, size: f32) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .child(div().mb(px(-0.25 * size)).child(top))
        .child(body)
        .into_any_element()
}

fn matrix(kind: MatrixKind, rows: &[Vec<Node>], size: f32, color: Hsla) -> AnyElement {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let mut grid = div()
        .grid()
        .grid_cols_max_content(columns as u16)
        .gap_x(px(match kind {
            MatrixKind::Aligned => 0.2 * size,
            _ => 0.7 * size,
        }))
        .gap_y(px(0.25 * size))
        .py(px(0.15 * size));
    for row in rows {
        for c in 0..columns {
            let cell = div().flex().flex_row().items_center();
            let cell = match (kind, c % 2) {
                (MatrixKind::Matrix, _) => cell.justify_center(),
                (MatrixKind::Cases, _) => cell.justify_start(),
                (MatrixKind::Aligned, 0) => cell.justify_end(),
                (MatrixKind::Aligned, _) => cell.justify_start(),
            };
            grid = grid.child(match row.get(c) {
                Some(node) => cell.child(element(node, size, color)),
                None => cell,
            });
        }
    }
    grid.into_any_element()
}

/// How tall `node` paints, in em of its own size — what sizes the delimiter
/// and the radical around it. An estimate from structure, since nothing has
/// been laid out yet.
fn height_em(node: &Node) -> f32 {
    match node {
        Node::Row(items) => {
            let lines: Vec<&[Node]> = items.split(|item| *item == Node::Break).collect();
            let line = |items: &[Node]| {
                items
                    .iter()
                    .map(height_em)
                    .fold(0.0_f32, f32::max)
                    .max(LINE)
            };
            if lines.len() > 1 {
                lines.iter().map(|l| line(l)).sum::<f32>() + 0.3 * (lines.len() - 1) as f32
            } else {
                line(items)
            }
        }
        Node::Atom(_) | Node::Op(_) | Node::Text(_) | Node::Space(_) | Node::Break => LINE,
        Node::Frac { num, den, .. } => height_em(num) + height_em(den) + 0.3,
        Node::Scripts { base, sup, sub } => {
            let scripts = sup
                .iter()
                .chain(sub.iter())
                .map(|s| height_em(s) * SCRIPT)
                .fold(0.0_f32, f32::max);
            height_em(base).max(1.5).max(scripts + 0.5)
        }
        Node::Sqrt { body, .. } => height_em(body) + 0.15,
        Node::BigOp { limits, sup, sub, .. } => {
            let mut h = BIG * 1.1;
            if *limits {
                h += sup.iter().chain(sub.iter()).count() as f32 * LINE * SCRIPT * 0.8;
            }
            h.max(1.5)
        }
        Node::Fenced { body, .. } => height_em(body),
        Node::Delim { scale, .. } => *scale,
        Node::Accent { body, .. } => height_em(body) + 0.2,
        Node::Matrix { rows, .. } => {
            let per_row = rows
                .iter()
                .map(|row| row.iter().map(height_em).fold(0.0_f32, f32::max).max(LINE))
                .sum::<f32>();
            per_row + 0.25 * rows.len().saturating_sub(1) as f32 + 0.3
        }
        Node::Over { top, body } => height_em(body) + height_em(top) * SCRIPT * 0.6,
        Node::Under { bottom, body } => height_em(body) + height_em(bottom) * SCRIPT * 0.6,
        Node::Overline(body) | Node::Underline(body) => height_em(body) + 0.15,
    }
}
