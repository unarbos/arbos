import XCTest
import SwiftUI
@testable import Arbos

/// Jacob, "write bubble sort" (Mac, 2026-10-26 10:50): the reply's `$O(n^2)$` showed its
/// dollars and caret verbatim. Math between `$…$`, `\(…\)`, `$$…$$` and `\[…\]` is typeset;
/// prices and a lone `$` stay prose. The vectors match `crates/arbos-mac/src/ui/math.rs`.
final class MathTextTests: XCTestCase {
    private func pieces(_ text: String) -> [MathText.Piece] { MathText.pieces(text) }

    func testBigOIsMathAndPricesAreNot() {
        XCTAssertEqual(pieces("It runs in $O(n^2)$ time."),
                       [.prose("It runs in "), .math("O(n^2)"), .prose(" time.")])
        XCTAssertEqual(pieces("costs $5 and $10"), [.prose("costs $5 and $10")])
        XCTAssertEqual(pieces("$5"), [.prose("$5")])
        XCTAssertEqual(pieces("US$3 and US$4"), [.prose("US$3 and US$4")])
        XCTAssertEqual(pieces("a lone $ sign"), [.prose("a lone $ sign")])
        XCTAssertEqual(pieces("unmatched $x here"), [.prose("unmatched $x here")])
        XCTAssertEqual(pieces("escaped \\$5 stays"), [.prose("escaped $5 stays")])
        XCTAssertEqual(pieces("`$x$` in code"), [.prose("`$x$` in code")])
        XCTAssertEqual(pieces("so \\(x^2\\) and $$y_1$$ too"),
                       [.prose("so "), .math("x^2"), .prose(" and "), .math("y_1"), .prose(" too")])
        XCTAssertEqual(pieces("no $ x$ math"), [.prose("no $ x$ math")])
    }

    /// Jacob's reply, byte for byte (the fixture's `AppModel.bubbleSortReply`).
    static let bubbleSort = """
    Here is a standard implementation of bubble sort in Python:

    ```python
    def bubble_sort(arr):
        n = len(arr)
        for i in range(n):
            swapped = False
            for j in range(0, n - i - 1):
                if arr[j] > arr[j + 1]:
                    arr[j], arr[j + 1] = arr[j + 1], arr[j]
                    swapped = True
            # If no two elements swapped, list is already sorted
            if not swapped:
                break
        return arr

    # Example usage:
    items = [64, 34, 25, 12, 22, 11, 90]
    print(bubble_sort(items))  # Output: [11, 12, 22, 25, 34, 64, 90]
    ```

    It runs in $O(n^2)$ time in the worst and average cases, and $O(n)$ time when the array is already sorted thanks to the `swapped` flag. Space complexity is $O(1)$.
    """

    func testTheBubbleSortReplyRendersItsComplexityAsMath() {
        let blocks = Markdown.blocks(Self.bubbleSort)
        XCTAssertEqual(blocks.count, 3)
        guard case .paragraph(let tail) = blocks[2] else { return XCTFail("not prose: \(blocks[2])") }
        let attr = Markdown.inline(tail, color: .white, link: .blue, codeChip: .gray)
        let text = String(attr.characters)
        XCTAssertTrue(text.contains("runs in O(n²) time"), text)
        XCTAssertTrue(text.contains("and O(n) time"), text)
        XCTAssertTrue(text.hasSuffix("Space complexity is O(1)."), text)
        XCTAssertFalse(text.contains("$"), text)
        XCTAssertEqual(Markdown.plain(tail), text)
        // The variables are italic serif, the digits and brackets upright.
        let fonts = attr.runs.compactMap { run -> (String, Font)? in
            guard let font = run.swiftUI.font else { return nil }
            return (String(attr[run.range].characters), font)
        }
        XCTAssertTrue(fonts.contains { $0.0 == "O" && $0.1 == Markdown.mathFont(Theme.body, .italic) }, "\(fonts)")
        XCTAssertTrue(fonts.contains { $0.0.hasPrefix("²)") && $0.1 == Markdown.mathFont(Theme.body, .upright) }, "\(fonts)")
        // The `swapped` code chip and the prose around the math survive.
        XCTAssertTrue(attr.runs.contains { $0.inlinePresentationIntent?.contains(.code) == true })
    }

    func testDisplayMathIsItsOwnCentredBlock() {
        let blocks = Markdown.blocks("Sum:\n\n$$\n\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}\n$$\n\nDone.")
        XCTAssertEqual(blocks, [.paragraph("Sum:"), .math(source: "\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}"), .paragraph("Done.")])
        XCTAssertEqual(Markdown.blocks("$$ E = mc^2 $$"), [.math(source: "E = mc^2")])
        XCTAssertEqual(Markdown.blocks("\\[\nx^2\n\\]"), [.math(source: "x^2")])
        // Never closed: prose as typed.
        XCTAssertEqual(Markdown.blocks("$$\nx^2\n"), [.paragraph("$$ x^2")])
        XCTAssertEqual(Quote.oneLine("$$ E = mc^2 $$"), "E = mc²")
        XCTAssertEqual(MathText.plain("\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}"), "∑ᵢ₌₁ⁿ\u{2009}i = (n(n + 1))/2")
    }

    func testTypesetVectorsMatchTheMac() {
        XCTAssertEqual(MathText.plain("O(n^2)"), "O(n²)")
        XCTAssertEqual(MathText.plain("O(n \\log n)"), "O(n\u{2009}log\u{2009}n)")
        XCTAssertEqual(MathText.plain("2x + \\sin(\\theta) = 1"), "2x + sin(θ) = 1")
        XCTAssertEqual(MathText.plain("\\sin x"), "sin\u{2009}x")
        XCTAssertEqual(MathText.plain("n-1"), "n − 1")
        XCTAssertEqual(MathText.plain("f(-x) = -f(x)"), "f(−x) = −f(x)")
        XCTAssertEqual(MathText.plain("a \\times b \\cdot c"), "a × b ⋅ c")
        XCTAssertEqual(MathText.plain("x \\leq y \\neq z \\to \\infty"), "x ≤ y ≠ z → ∞")
        XCTAssertEqual(MathText.plain("x^{n-1}"), "xⁿ⁻¹")
        XCTAssertEqual(MathText.plain("x_1 + x_2"), "x₁ + x₂")
        XCTAssertEqual(MathText.plain("e^{i\\pi} + 1 = 0"), "e^(iπ) + 1 = 0")
        XCTAssertEqual(MathText.plain("x^{a^b}"), "x^(a^b)")
        XCTAssertEqual(MathText.plain("\\frac{1}{2}"), "¹⁄₂")
        XCTAssertEqual(MathText.plain("\\frac{a}{b}"), "a/b")
        XCTAssertEqual(MathText.plain("\\frac{(a+b)}{2}"), "(a + b)/2")
        XCTAssertEqual(MathText.plain("\\sqrt{x^2 + y^2}"), "√(x² + y²)")
        XCTAssertEqual(MathText.plain("\\sqrt[3]{x}"), "∛x")
        XCTAssertEqual(MathText.plain("\\text{cost} = 5"), "cost = 5")
        XCTAssertEqual(MathText.plain("\\mathbf{v} \\in \\mathbb{R}^n"), "v ∈ ℝⁿ")
        XCTAssertEqual(MathText.plain("\\left( \\frac{a}{b} \\right)"), "(a/b)")
        XCTAssertEqual(MathText.plain("\\foo x"), "foox")
        XCTAssertEqual(MathText.plain(""), "")
        XCTAssertEqual(MathText.typeset("O(n^2)"), [
            .init(text: "O", style: .italic), .init(text: "(", style: .upright),
            .init(text: "n", style: .italic), .init(text: "²)", style: .upright),
        ])
        XCTAssertEqual(MathText.typeset("\\text{if } x > 0"), [
            .init(text: "if ", style: .upright), .init(text: "x", style: .italic), .init(text: " > 0", style: .upright),
        ])
        XCTAssertEqual(MathText.typeset("\\mathbf{v}"), [.init(text: "v", style: .bold)])
    }
}
