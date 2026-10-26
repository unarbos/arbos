// A small LaTeX subset typeset as text runs: what `$O(n^2)$` or `\( \sum_{i=1}^{n} i \)`
// in an agent reply becomes on screen. No layout engine — superscripts and subscripts use
// Unicode's own small glyphs, a fraction is `a/b`, a root is `√x` — but letters are italic in
// a serif math face and digits, operators and function names upright, the way KaTeX paints
// Grok Bot's replies ([feedback math-render-ios-2026-10-26]).
//
// The Mac app carries the same subset in `crates/arbos-mac/src/ui/math.rs`; keep the two in
// step (`MathTextTests` holds the shared vectors).

import Foundation

enum MathText {
    /// How one run of typeset math is painted.
    enum Style: Equatable {
        /// Variables: Latin letters and lowercase Greek.
        case italic
        /// Digits, operators, brackets, function names, `\text{…}`.
        case upright
        /// `\mathbf{…}` / `\boldsymbol{…}`.
        case bold
    }

    /// One run of the typeset text.
    struct Run: Equatable {
        var text: String
        var style: Style
    }

    /// `src` (the LaTeX between the delimiters) as styled runs, adjacent runs of one style
    /// merged. Never fails: unknown commands come out as their name.
    static func typeset(_ src: String) -> [Run] {
        var parser = Parser(chars: Array(src), scriptDepth: 0)
        var out = Out()
        parser.sequence(&out, mode: .math, inGroup: false)
        return out.finish()
    }

    /// The typeset text with no styling: what a selection copies and what sizes a column.
    static func plain(_ src: String) -> String {
        typeset(src).map(\.text).joined()
    }

    // MARK: - Delimiters

    /// An inline math span starting at `chars[i]`: the LaTeX inside and the index after the
    /// closing delimiter. `\( … \)`, `\[ … \]` and `$$ … $$` always; `$ … $` by Pandoc's rule
    /// so prices stay prose: the opening `$` is not glued to a word and has a non-space right
    /// after it, the closing `$` has a non-space right before it and no digit after it, and
    /// the span stays on one line — "$5 and $10", "US$3" and a lone "$" are text.
    static func span(in chars: [Character], at i: Int) -> (source: String, end: Int)? {
        func inner(_ from: Int, _ to: Int) -> String { String(chars[from..<to]) }
        func findPair(_ from: Int, _ a: Character, _ b: Character) -> Int? {
            var j = from
            while j + 1 < chars.count {
                if chars[j] == a && chars[j + 1] == b { return j }
                j += 1
            }
            return nil
        }
        if chars[i] == "\\" {
            guard i + 1 < chars.count else { return nil }
            let close: Character
            switch chars[i + 1] {
            case "(": close = ")"
            case "[": close = "]"
            default: return nil
            }
            guard let end = findPair(i + 2, "\\", close) else { return nil }
            let source = inner(i + 2, end).trimmingCharacters(in: .whitespacesAndNewlines)
            return source.isEmpty ? nil : (source, end + 2)
        }
        guard chars[i] == "$" else { return nil }
        if i + 1 < chars.count, chars[i + 1] == "$" {
            guard let end = findPair(i + 2, "$", "$") else { return nil }
            let source = inner(i + 2, end).trimmingCharacters(in: .whitespacesAndNewlines)
            return source.isEmpty ? nil : (source, end + 2)
        }
        if i > 0, chars[i - 1].isLetter || chars[i - 1].isNumber { return nil }
        guard i + 1 < chars.count, !chars[i + 1].isWhitespace else { return nil }
        var j = i + 1
        while j < chars.count {
            if chars[j] == "\n" { return nil }
            if chars[j] == "$", chars[j - 1] != "\\" { break }
            j += 1
        }
        guard j < chars.count, !chars[j - 1].isWhitespace else { return nil }
        if j + 1 < chars.count, chars[j + 1].isNumber { return nil }
        return (inner(i + 1, j), j + 1)
    }

    /// `text` cut into prose and math: `.math` pieces carry the LaTeX source, `.prose` pieces
    /// the text between them with `\$` unescaped. No math → one prose piece.
    enum Piece: Equatable {
        case prose(String)
        case math(String)
    }

    static func pieces(_ text: String) -> [Piece] {
        let chars = Array(text)
        var out: [Piece] = []
        var current = ""
        var i = 0
        var inCode = false
        while i < chars.count {
            let c = chars[i]
            if c == "`" {
                inCode.toggle()
                current.append(c)
                i += 1
                continue
            }
            if inCode {
                current.append(c)
                i += 1
                continue
            }
            if c == "\\", i + 1 < chars.count, chars[i + 1] == "$" {
                current.append("$")
                i += 2
                continue
            }
            if let (source, end) = span(in: chars, at: i) {
                if !current.isEmpty { out.append(.prose(current)); current = "" }
                out.append(.math(source))
                i = end
                continue
            }
            current.append(c)
            i += 1
        }
        if !current.isEmpty || out.isEmpty { out.append(.prose(current)) }
        return out
    }

    /// The opener a display-math line starts with and the closer that ends it.
    static func displayDelimiters(_ line: String) -> (opener: String, closer: String)? {
        let t = line.drop { $0.isWhitespace }
        if t.hasPrefix("$$") { return ("$$", "$$") }
        if t.hasPrefix("\\[") { return ("\\[", "\\]") }
        return nil
    }

    // MARK: - Typesetting

    /// Letter treatment inside the current group.
    enum Mode {
        case math
        /// `\mathrm{…}`: letters upright.
        case upright
        /// `\mathbf{…}`: everything bold.
        case bold

        var letter: Style {
            switch self {
            case .math: return .italic
            case .upright: return .upright
            case .bold: return .bold
            }
        }

        var fixed: Style {
            switch self {
            case .math, .upright: return .upright
            case .bold: return .bold
            }
        }
    }

    struct Out {
        var runs: [Run] = []
        /// The last thing pushed can take a binary operator after it (an operand, not an
        /// operator or an opening bracket): `a - b` spaces the minus, `(-1)` does not.
        var operandBefore = false
        /// A function name (`\sin`) was just pushed: a thin space follows unless a bracket does.
        var afterFunction = false

        mutating func push(_ text: String, _ style: Style) {
            guard !text.isEmpty else { return }
            if afterFunction {
                afterFunction = false
                if let first = text.first, !"([{|".contains(first) {
                    raw("\u{2009}", .upright)
                }
            }
            raw(text, style)
            if let last = text.last {
                operandBefore = !"([{ ,\u{2009}".contains(last)
            }
        }

        mutating func raw(_ text: String, _ style: Style) {
            if let last = runs.indices.last, runs[last].style == style {
                runs[last].text += text
                return
            }
            runs.append(Run(text: text, style: style))
        }

        /// `op` with a space on each side when it sits between two operands (`a + b`,
        /// `x = 1`), bare when it is unary (`-1`) or inside a script.
        mutating func `operator`(_ op: String, spaced: Bool) {
            if spaced && operandBefore {
                push(" ", .upright)
                push(op, .upright)
                push(" ", .upright)
            } else {
                push(op, .upright)
            }
            operandBefore = false
        }

        var plain: String { runs.map(\.text).joined() }

        mutating func finish() -> [Run] {
            if let last = runs.indices.last {
                while runs[last].text.last?.isWhitespace == true { runs[last].text.removeLast() }
            }
            if !runs.isEmpty {
                while runs[0].text.first?.isWhitespace == true { runs[0].text.removeFirst() }
            }
            return runs.filter { !$0.text.isEmpty }
        }
    }

    struct Parser {
        let chars: [Character]
        var i = 0
        /// > 0 while typesetting the argument of `^` or `_`: no operator spacing, nested
        /// scripts fall back to `^`/`_` text.
        var scriptDepth: Int

        init(chars: [Character], scriptDepth: Int) {
            self.chars = chars
            self.scriptDepth = scriptDepth
        }

        var peek: Character? { i < chars.count ? chars[i] : nil }
        var spaced: Bool { scriptDepth == 0 }

        mutating func skipSpaces() {
            while let c = peek, c.isWhitespace { i += 1 }
        }

        var peekAfterSpaces: Character? {
            var j = i
            while j < chars.count, chars[j].isWhitespace { j += 1 }
            return j < chars.count ? chars[j] : nil
        }

        /// Typeset until the end, or until the `}` closing an open group.
        mutating func sequence(_ out: inout Out, mode: Mode, inGroup: Bool) {
            while let c = peek {
                switch c {
                case "}":
                    i += 1
                    if inGroup { return }
                case "{":
                    i += 1
                    sequence(&out, mode: mode, inGroup: true)
                case "\\":
                    command(&out, mode: mode)
                case "^", "_":
                    i += 1
                    let arg = scriptArgument(mode: mode)
                    script(&out, up: c == "^", arg: arg)
                case "&":
                    i += 1
                    out.push(" ", .upright)
                case _ where c.isWhitespace:
                    i += 1
                default:
                    i += 1
                    symbol(&out, c, mode: mode)
                }
            }
        }

        /// One plain character of the source.
        mutating func symbol(_ out: inout Out, _ c: Character, mode: Mode) {
            let fixed = mode.fixed
            switch c {
            case _ where c.isLetter: out.push(String(c), mode.letter)
            case "-": out.operator("\u{2212}", spaced: spaced)
            case "+": out.operator("+", spaced: spaced)
            case "=", "<", ">": out.operator(String(c), spaced: spaced)
            case "*": out.operator("\u{2217}", spaced: spaced)
            case "'": out.push("\u{2032}", fixed)
            case ",", ";":
                out.push(String(c), fixed)
                if spaced { out.push(" ", fixed) }
            default: out.push(String(c), fixed)
            }
        }

        /// `\name` or `\<one char>`: the name without the backslash.
        mutating func commandName() -> String {
            i += 1
            var name = ""
            guard let first = peek else { return name }
            if first.isASCII, first.isLetter {
                while let c = peek, c.isASCII, c.isLetter {
                    name.append(c)
                    i += 1
                }
            } else {
                name.append(first)
                i += 1
            }
            return name
        }

        /// The next argument's source: a `{…}` group, a command, or one character.
        mutating func argumentSource() -> String {
            skipSpaces()
            guard let c = peek else { return "" }
            switch c {
            case "{":
                i += 1
                let start = i
                var depth = 1
                while let c = peek {
                    if c == "{" { depth += 1 }
                    if c == "}" {
                        depth -= 1
                        if depth == 0 { break }
                    }
                    i += 1
                }
                let src = String(chars[start..<i])
                if peek == "}" { i += 1 }
                return src
            case "\\":
                let start = i
                _ = commandName()
                return String(chars[start..<i])
            default:
                i += 1
                return String(c)
            }
        }

        func subTypeset(_ src: String, mode: Mode, script: Bool) -> [Run] {
            var p = Parser(chars: Array(src), scriptDepth: scriptDepth + (script ? 1 : 0))
            var out = Out()
            p.sequence(&out, mode: mode, inGroup: false)
            return out.finish()
        }

        mutating func scriptArgument(mode: Mode) -> [Run] {
            let src = argumentSource()
            return subTypeset(src, mode: mode, script: true)
        }

        /// A superscript or subscript: Unicode's small glyphs when every character has one,
        /// else `^x` / `_(xy)` in normal text.
        func script(_ out: inout Out, up: Bool, arg: [Run]) {
            let plain = arg.map(\.text).joined()
            guard !plain.isEmpty else { return }
            var mapped: [Run]? = []
            for r in arg {
                var text = ""
                for c in r.text {
                    guard let small = up ? MathText.superscript(c) : MathText.subscript(c) else {
                        mapped = nil
                        break
                    }
                    text.append(small)
                }
                guard mapped != nil else { break }
                mapped?.append(Run(text: text, style: r.style))
            }
            // The thin space after `\sum` / `\sin` belongs after the scripts, not before them.
            let afterFunction = out.afterFunction
            out.afterFunction = false
            if let runs = mapped, scriptDepth == 0 {
                for r in runs { out.push(r.text, r.style) }
            } else {
                out.push(up ? "^" : "_", .upright)
                let wrap = plain.count > 1
                if wrap { out.push("(", .upright) }
                for r in arg { out.push(r.text, r.style) }
                if wrap { out.push(")", .upright) }
            }
            out.afterFunction = afterFunction
            out.operandBefore = true
        }

        mutating func command(_ out: inout Out, mode: Mode) {
            let name = commandName()
            let fixed = mode.fixed
            switch name {
            case "": break
            case ",", ":", ";", " ", "quad", "qquad", "~": out.push(" ", fixed)
            case "!": break
            case "\\": out.push("\n", fixed)
            case "left", "right", "big", "Big", "bigg", "Bigg", "bigl", "bigr", "Bigl", "Bigr",
                 "biggl", "biggr", "limits", "nolimits", "displaystyle", "textstyle", "scriptstyle",
                 "scriptscriptstyle":
                if (name == "left" || name == "right"), peek == "." { i += 1 }
            case "{", "}", "%", "$", "#", "&", "_": out.push(name, fixed)
            case "|": out.push("\u{2016}", fixed)
            case "text", "textrm", "operatorname", "mbox":
                out.push(argumentSource(), fixed)
                if name == "operatorname" { out.afterFunction = true }
            case "textbf":
                out.push(argumentSource(), .bold)
            case "textit", "mathit":
                for r in subTypeset(argumentSource(), mode: .math, script: false) { out.push(r.text, .italic) }
            case "mathrm":
                for r in subTypeset(argumentSource(), mode: .upright, script: false) { out.push(r.text, r.style) }
            case "mathbf", "boldsymbol", "bm", "pmb":
                for r in subTypeset(argumentSource(), mode: .bold, script: false) { out.push(r.text, r.style) }
            case "mathbb":
                out.push(String(argumentSource().map(MathText.blackboard)), fixed)
            case "mathcal", "mathscr", "mathfrak", "mathsf", "mathtt":
                for r in subTypeset(argumentSource(), mode: mode, script: false) { out.push(r.text, r.style) }
            case "frac", "dfrac", "tfrac":
                let a = argumentSource()
                let b = argumentSource()
                fraction(&out, mode: mode, a, b)
            case "binom":
                let a = argumentSource()
                let b = argumentSource()
                out.push("C(", fixed)
                for r in subTypeset(a, mode: mode, script: false) { out.push(r.text, r.style) }
                out.push(", ", fixed)
                for r in subTypeset(b, mode: mode, script: false) { out.push(r.text, r.style) }
                out.push(")", fixed)
            case "sqrt":
                var index = ""
                if peekAfterSpaces == "[" {
                    skipSpaces()
                    i += 1
                    let start = i
                    while let c = peek, c != "]" { i += 1 }
                    index = String(chars[start..<i])
                    if peek == "]" { i += 1 }
                }
                let src = argumentSource()
                let radical: String
                switch index.trimmingCharacters(in: .whitespaces) {
                case "", "2": radical = "\u{221A}"
                case "3": radical = "\u{221B}"
                case "4": radical = "\u{221C}"
                case let other:
                    let small = other.compactMap(MathText.superscript)
                    radical = (small.count == other.count ? String(small) : other) + "\u{221A}"
                }
                out.push(radical, fixed)
                let inner = subTypeset(src, mode: mode, script: false)
                let plain = inner.map(\.text).joined()
                let wrap = plain.count > 1 && !MathText.enclosed(plain)
                if wrap { out.push("(", fixed) }
                for r in inner { out.push(r.text, r.style) }
                if wrap { out.push(")", fixed) }
            case "hat", "bar", "vec", "dot", "ddot", "tilde", "overline", "underline", "widehat",
                 "widetilde", "overrightarrow":
                let mark: Character
                switch name {
                case "hat", "widehat": mark = "\u{0302}"
                case "bar": mark = "\u{0304}"
                case "vec", "overrightarrow": mark = "\u{20D7}"
                case "dot": mark = "\u{0307}"
                case "ddot": mark = "\u{0308}"
                case "tilde", "widetilde": mark = "\u{0303}"
                case "overline": mark = "\u{0305}"
                default: mark = "\u{0332}"
                }
                for r in subTypeset(argumentSource(), mode: mode, script: false) {
                    // Combining marks join the base letter into one grapheme; append by scalar so the
                    // mark lands on every character.
                    var text = ""
                    for c in r.text {
                        text.unicodeScalars.append(contentsOf: c.unicodeScalars)
                        text.unicodeScalars.append(contentsOf: mark.unicodeScalars)
                    }
                    out.push(text, r.style)
                }
            case "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh",
                 "tanh", "coth", "log", "ln", "lg", "exp", "lim", "limsup", "liminf", "max", "min", "sup",
                 "inf", "det", "gcd", "arg", "deg", "dim", "ker", "hom", "Pr":
                if out.operandBefore { out.push("\u{2009}", fixed) }
                out.push(name, fixed)
                out.afterFunction = true
            default:
                if let sym = MathText.relation(name) {
                    out.operator(sym, spaced: spaced)
                } else if let sym = MathText.binary(name) {
                    out.operator(sym, spaced: spaced)
                } else if let (sym, italic) = MathText.symbol(name) {
                    out.push(sym, italic && mode == .math ? .italic : fixed)
                    if ["sum", "prod", "int", "iint", "oint", "coprod", "bigcup", "bigcap"].contains(name) {
                        out.afterFunction = true
                    }
                } else {
                    out.push(name, fixed)
                }
            }
        }

        /// `\frac{a}{b}`: `¹⁄₂` for digits, otherwise `a/b` with a side in parentheses when it
        /// has more than one term.
        func fraction(_ out: inout Out, mode: Mode, _ a: String, _ b: String) {
            let fixed = mode.fixed
            let top = subTypeset(a, mode: mode, script: false)
            let bottom = subTypeset(b, mode: mode, script: false)
            let topPlain = top.map(\.text).joined()
            let bottomPlain = bottom.map(\.text).joined()
            func digits(_ s: String) -> Bool { !s.isEmpty && s.allSatisfy { $0.isASCII && $0.isNumber } }
            if digits(topPlain), digits(bottomPlain) {
                let small = String(topPlain.compactMap(MathText.superscript)) + "\u{2044}"
                    + String(bottomPlain.compactMap(MathText.subscript))
                out.push(small, fixed)
                return
            }
            func side(_ out: inout Out, _ runs: [Run], _ plain: String) {
                let wrap = MathText.needsParens(plain)
                if wrap { out.push("(", fixed) }
                for r in runs { out.push(r.text, r.style) }
                if wrap { out.push(")", fixed) }
            }
            side(&out, top, topPlain)
            out.push("/", fixed)
            out.operandBefore = false
            side(&out, bottom, bottomPlain)
        }
    }

    // MARK: - Tables

    /// A fraction side or a root's content that reads wrong without brackets: more than one
    /// term, and not already bracketed as a whole.
    static func needsParens(_ plain: String) -> Bool {
        if plain.count <= 1 || enclosed(plain) { return false }
        return plain.contains { " +\u{2212}\u{00B1}\u{00D7}\u{22C5}/".contains($0) }
    }

    /// `(…)` / `[…]` / `{…}` / `|…|` around the whole string.
    static func enclosed(_ plain: String) -> Bool {
        guard let first = plain.first, let last = plain.last, plain.count >= 2 else { return false }
        let pairs: [Character: Character] = ["(": ")", "[": "]", "{": "}", "|": "|", "\u{2016}": "\u{2016}"]
        guard pairs[first] == last else { return false }
        if first == "|" || first == "\u{2016}" { return true }
        var depth = 0
        let count = plain.count
        for (idx, c) in plain.enumerated() {
            if c == first { depth += 1 } else if c == last {
                depth -= 1
                if depth == 0, idx + 1 < count { return false }
            }
        }
        return depth == 0
    }

    static func superscript(_ c: Character) -> Character? {
        let map: [Character: Character] = [
            "0": "\u{2070}", "1": "\u{00B9}", "2": "\u{00B2}", "3": "\u{00B3}", "4": "\u{2074}",
            "5": "\u{2075}", "6": "\u{2076}", "7": "\u{2077}", "8": "\u{2078}", "9": "\u{2079}",
            "+": "\u{207A}", "\u{2212}": "\u{207B}", "-": "\u{207B}", "=": "\u{207C}",
            "(": "\u{207D}", ")": "\u{207E}",
            "a": "\u{1D43}", "b": "\u{1D47}", "c": "\u{1D9C}", "d": "\u{1D48}", "e": "\u{1D49}",
            "f": "\u{1DA0}", "g": "\u{1D4D}", "h": "\u{02B0}", "i": "\u{2071}", "j": "\u{02B2}",
            "k": "\u{1D4F}", "l": "\u{02E1}", "m": "\u{1D50}", "n": "\u{207F}", "o": "\u{1D52}",
            "p": "\u{1D56}", "r": "\u{02B3}", "s": "\u{02E2}", "t": "\u{1D57}", "u": "\u{1D58}",
            "v": "\u{1D5B}", "w": "\u{02B7}", "x": "\u{02E3}", "y": "\u{02B8}", "z": "\u{1DBB}",
            "T": "\u{1D40}", "N": "\u{1D3A}", "\u{2032}": "\u{2032}", "\u{00B0}": "\u{00B0}",
        ]
        return map[c]
    }

    static func `subscript`(_ c: Character) -> Character? {
        let map: [Character: Character] = [
            "0": "\u{2080}", "1": "\u{2081}", "2": "\u{2082}", "3": "\u{2083}", "4": "\u{2084}",
            "5": "\u{2085}", "6": "\u{2086}", "7": "\u{2087}", "8": "\u{2088}", "9": "\u{2089}",
            "+": "\u{208A}", "\u{2212}": "\u{208B}", "-": "\u{208B}", "=": "\u{208C}",
            "(": "\u{208D}", ")": "\u{208E}",
            "a": "\u{2090}", "e": "\u{2091}", "h": "\u{2095}", "i": "\u{1D62}", "j": "\u{2C7C}",
            "k": "\u{2096}", "l": "\u{2097}", "m": "\u{2098}", "n": "\u{2099}", "o": "\u{2092}",
            "p": "\u{209A}", "r": "\u{1D63}", "s": "\u{209B}", "t": "\u{209C}", "u": "\u{1D64}",
            "v": "\u{1D65}", "x": "\u{2093}",
        ]
        return map[c]
    }

    static func blackboard(_ c: Character) -> Character {
        switch c {
        case "R": return "\u{211D}"
        case "N": return "\u{2115}"
        case "Z": return "\u{2124}"
        case "Q": return "\u{211A}"
        case "C": return "\u{2102}"
        case "P": return "\u{2119}"
        case "H": return "\u{210D}"
        default: return c
        }
    }

    /// Relations (`=`-like spacing).
    static func relation(_ name: String) -> String? {
        let map: [String: String] = [
            "leq": "\u{2264}", "le": "\u{2264}", "leqslant": "\u{2264}",
            "geq": "\u{2265}", "ge": "\u{2265}", "geqslant": "\u{2265}",
            "neq": "\u{2260}", "ne": "\u{2260}", "approx": "\u{2248}", "equiv": "\u{2261}",
            "sim": "\u{223C}", "simeq": "\u{2243}", "cong": "\u{2245}", "propto": "\u{221D}",
            "ll": "\u{226A}", "gg": "\u{226B}",
            "to": "\u{2192}", "rightarrow": "\u{2192}", "leftarrow": "\u{2190}", "gets": "\u{2190}",
            "leftrightarrow": "\u{2194}", "Rightarrow": "\u{21D2}", "implies": "\u{21D2}",
            "Leftarrow": "\u{21D0}", "Leftrightarrow": "\u{21D4}", "iff": "\u{21D4}", "mapsto": "\u{21A6}",
            "in": "\u{2208}", "notin": "\u{2209}", "ni": "\u{220B}",
            "subset": "\u{2282}", "subseteq": "\u{2286}", "supset": "\u{2283}", "supseteq": "\u{2287}",
            "perp": "\u{22A5}", "parallel": "\u{2225}", "mid": "|", "models": "\u{22A8}", "vdash": "\u{22A2}",
        ]
        return map[name]
    }

    /// Binary operators (`+`-like spacing).
    static func binary(_ name: String) -> String? {
        let map: [String: String] = [
            "times": "\u{00D7}", "cdot": "\u{22C5}", "div": "\u{00F7}", "pm": "\u{00B1}", "mp": "\u{2213}",
            "cup": "\u{222A}", "cap": "\u{2229}", "setminus": "\u{2216}",
            "land": "\u{2227}", "wedge": "\u{2227}", "lor": "\u{2228}", "vee": "\u{2228}",
            "oplus": "\u{2295}", "otimes": "\u{2297}", "circ": "\u{2218}", "bullet": "\u{2219}",
            "star": "\u{22C6}", "ast": "\u{2217}",
        ]
        return map[name]
    }

    /// Ordinary symbols and Greek letters: the glyph and whether it is a variable (italic,
    /// like TeX's lowercase Greek).
    static func symbol(_ name: String) -> (String, Bool)? {
        let variables: [String: String] = [
            "alpha": "\u{03B1}", "beta": "\u{03B2}", "gamma": "\u{03B3}", "delta": "\u{03B4}",
            "epsilon": "\u{03F5}", "varepsilon": "\u{03B5}", "zeta": "\u{03B6}", "eta": "\u{03B7}",
            "theta": "\u{03B8}", "vartheta": "\u{03D1}", "iota": "\u{03B9}", "kappa": "\u{03BA}",
            "lambda": "\u{03BB}", "mu": "\u{03BC}", "nu": "\u{03BD}", "xi": "\u{03BE}", "pi": "\u{03C0}",
            "varpi": "\u{03D6}", "rho": "\u{03C1}", "varrho": "\u{03F1}", "sigma": "\u{03C3}",
            "varsigma": "\u{03C2}", "tau": "\u{03C4}", "upsilon": "\u{03C5}", "phi": "\u{03D5}",
            "varphi": "\u{03C6}", "chi": "\u{03C7}", "psi": "\u{03C8}", "omega": "\u{03C9}",
            "hbar": "\u{210F}", "ell": "\u{2113}", "wp": "\u{2118}",
        ]
        if let v = variables[name] { return (v, true) }
        let fixed: [String: String] = [
            "Gamma": "\u{0393}", "Delta": "\u{0394}", "Theta": "\u{0398}", "Lambda": "\u{039B}",
            "Xi": "\u{039E}", "Pi": "\u{03A0}", "Sigma": "\u{03A3}", "Upsilon": "\u{03A5}",
            "Phi": "\u{03A6}", "Psi": "\u{03A8}", "Omega": "\u{03A9}",
            "infty": "\u{221E}", "partial": "\u{2202}", "nabla": "\u{2207}",
            "sum": "\u{2211}", "prod": "\u{220F}", "int": "\u{222B}", "iint": "\u{222C}", "oint": "\u{222E}",
            "forall": "\u{2200}", "exists": "\u{2203}", "nexists": "\u{2204}",
            "emptyset": "\u{2205}", "varnothing": "\u{2205}", "neg": "\u{00AC}", "lnot": "\u{00AC}",
            "ldots": "\u{2026}", "dots": "\u{2026}", "hdots": "\u{2026}", "cdots": "\u{22EF}",
            "vdots": "\u{22EE}", "ddots": "\u{22F1}", "prime": "\u{2032}", "degree": "\u{00B0}",
            "angle": "\u{2220}", "triangle": "\u{25B3}", "Re": "\u{211C}", "Im": "\u{2111}",
            "aleph": "\u{2135}", "lceil": "\u{2308}", "rceil": "\u{2309}", "lfloor": "\u{230A}",
            "rfloor": "\u{230B}", "langle": "\u{27E8}", "rangle": "\u{27E9}",
            "lvert": "|", "rvert": "|", "vert": "|", "lVert": "\u{2016}", "rVert": "\u{2016}", "Vert": "\u{2016}",
            "checkmark": "\u{2713}", "top": "\u{22A4}", "bot": "\u{22A5}",
            "therefore": "\u{2234}", "because": "\u{2235}",
        ]
        if let f = fixed[name] { return (f, false) }
        return nil
    }
}
