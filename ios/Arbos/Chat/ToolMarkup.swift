import Foundation

/// Tool calls a model wrote as text instead of calling (`<invoke>`,
/// `<function_calls>`, `<tool_call>`, `<function=…>`, Llama's markers).
/// The kernel cuts them from the settled line (#278), but the live stream
/// carries the raw tokens as they arrive, and a reply that was nothing but
/// markup settles to no line at all — so the phone hides the markup while
/// it streams and strips it when the bubble closes. Mirrors
/// `arbos_engine::markup::strip_tool_markup`.
enum ToolMarkup {
    /// Element openers and their closers, longest opener first so that
    /// `<function_calls` is not read as `<function_call`.
    private static let elements: [(open: String, close: String)] = [
        ("<function_calls", "</function_calls>"),
        ("<function_call", "</function_call>"),
        ("<function=", "</function>"),
        ("<tool_calls", "</tool_calls>"),
        ("<tool_call", "</tool_call>"),
        ("<invoke", "</invoke>"),
    ]
    /// Markers with no closer: everything after them is the call.
    private static let markers = ["<|python_tag|>", "<|tool_call|>", "[TOOL_CALLS]"]
    private static let strayClosers = ["</function_calls>", "</function_call>", "</function>", "</tool_calls>", "</tool_call>", "</invoke>", "</parameter>"]

    /// Where the first piece of markup starts, if any.
    static func firstOpener(in text: String) -> String.Index? {
        var earliest: String.Index?
        for token in elements.map(\.open) + markers {
            if let range = text.range(of: token), earliest.map({ range.lowerBound < $0 }) ?? true {
                earliest = range.lowerBound
            }
        }
        return earliest
    }

    /// What a streaming bubble shows: the prose before the first opener.
    static func visible(_ text: String) -> String {
        guard let start = firstOpener(in: text) else { return text }
        return String(text[..<start]).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// The reply without its markup: each element from its opener to its
    /// closer (or to the end when unclosed), each marker with what follows,
    /// stray closers, and the blank lines the cuts left.
    static func strip(_ text: String) -> String {
        var out = text
        var guardCount = 0
        while let start = firstOpener(in: out), guardCount < 64 {
            guardCount += 1
            let tail = out[start...]
            var end = out.endIndex
            if let element = elements.first(where: { tail.hasPrefix($0.open) }) {
                if let close = tail.range(of: element.close) { end = close.upperBound }
            }
            out.removeSubrange(start..<end)
        }
        for closer in strayClosers {
            out = out.replacingOccurrences(of: closer, with: "")
        }
        while out.contains("\n\n\n") {
            out = out.replacingOccurrences(of: "\n\n\n", with: "\n\n")
        }
        return out.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
