import SwiftUI

/// The transcript must never come back blank while the chat has rows
/// ([feedback iphone-chat-empties-after-screenshot-2026-10-28]: "Sometimes after taking a
/// screenshot the chat empties entirely" — header, presence row and composer there, no
/// bubble). A system screenshot opens the feedback sheet over the chat and the phone goes
/// inactive and back while the thumbnail is handled; the transcript is a lazy list anchored
/// at its end, and SwiftUI's lazy container can come out of a covered / inactive spell with
/// no realised row near its viewport. The data is intact (`rows`); nothing is drawn until the
/// reader scrolls. Two pure rules here, one object for the live scroll geometry:
///
/// - `window(_:count:skipEnd:)` cuts the slice the view tree holds and is never empty while
///   there are rows — a window that slid into history and lost its rows re-seeds to the tail.
/// - `verdict(...)` reads the realised rows against the visible rect after a return and says
///   whether something is drawn, and if not where the reader lands: the tail when they were
///   following it, else the realised row nearest the viewport, kept exactly where it was.
enum TranscriptReanchor {
    /// The newest `count` rows of `all`, less the `skipEnd` newest when the window slid into
    /// history. Never empty while `all` has rows: a slide past the end, or a count that
    /// reached zero, falls back to the newest `fallback` rows.
    static func window<T>(_ all: [T], count: Int, skipEnd: Int, fallback: Int) -> [T] {
        guard !all.isEmpty else { return [] }
        let skip = max(0, skipEnd)
        let kept = skip > 0 && skip < all.count ? all.dropLast(skip) : all[...]
        let n = count > 0 ? count : fallback
        let cut = kept.count > n ? Array(kept.suffix(n)) : Array(kept)
        return cut.isEmpty ? Array(all.suffix(max(1, fallback))) : cut
    }

    /// A row the lazy list has realised, with its frame in the transcript's content space.
    struct Realized: Equatable {
        let id: String
        let frame: CGRect
    }

    /// What a return finds, and what to do about it.
    enum Verdict: Equatable {
        /// At least one realised row sits in the readable part of the viewport.
        case drawn
        /// Nothing drawn and the reader was following the end — land on the tail.
        case toTail
        /// Nothing drawn in the viewport, but `rowId` is realised: scroll it into view with
        /// `anchor` (exact when it already overlapped the viewport, so nothing visibly moves).
        case toRow(id: String, anchor: UnitPoint)
    }

    /// `visibleRect` is the scroll view's visible rect in content space (nil before iOS 18 or
    /// before the first sample); `headerInset` / `footerInset` are the chrome drawn over the
    /// transcript's top and bottom edges.
    static func verdict(rowsEmpty: Bool, pinned: Bool, realized: [Realized], visibleRect: CGRect?,
                        headerInset: CGFloat, footerInset: CGFloat) -> Verdict {
        if rowsEmpty { return .drawn }
        guard let visibleRect else {
            // No geometry to judge against: realised rows are the only signal.
            return realized.isEmpty ? .toTail : .drawn
        }
        var readable = visibleRect
        readable.origin.y += headerInset
        readable.size.height = max(0, readable.height - headerInset - footerInset)
        if realized.contains(where: { $0.frame.height > 0 && $0.frame.intersects(readable) }) { return .drawn }
        if pinned || realized.isEmpty { return .toTail }
        // Reading history with realised rows out of view: the nearest one comes back, where it was
        // when it still overlapped the viewport, else at the edge it is beyond.
        let mid = visibleRect.midY
        let nearest = realized.min { abs($0.frame.midY - mid) < abs($1.frame.midY - mid) }!
        if nearest.frame.intersects(visibleRect) {
            return .toRow(id: nearest.id, anchor: keepingAnchor(row: nearest.frame, viewport: visibleRect))
        }
        return .toRow(id: nearest.id, anchor: nearest.frame.midY < mid ? .top : .bottom)
    }

    /// The `scrollTo` anchor that leaves `row` exactly where it is: `scrollTo(id, anchor: a)`
    /// aligns the row's point at fraction `a` with the viewport's point at the same fraction,
    /// so a row whose top sits `d` below the viewport top keeps its place with a = d / (H − h).
    static func keepingAnchor(row: CGRect, viewport: CGRect) -> UnitPoint {
        let d = row.minY - viewport.minY
        let span = viewport.height - row.height
        guard span > 0.5 else { return .top }
        return UnitPoint(x: 0.5, y: min(1, max(0, d / span)))
    }
}

/// The transcript's live scroll geometry, written by the scroll view per sample and read on a
/// return. A plain class (like `RowFrameIndex`): writes invalidate no view.
final class TranscriptViewport {
    /// Visible rect in the scroll content's space; nil before the first sample (and on iOS 17).
    var visibleRect: CGRect?
}

/// Records the scroll view's visible rect into `viewport` (iOS 18+; earlier systems leave it nil).
struct TranscriptViewportReporter: ViewModifier {
    let viewport: TranscriptViewport

    func body(content: Content) -> some View {
        if #available(iOS 18, *) {
            content.onScrollGeometryChange(for: CGRect.self) { $0.visibleRect } action: { _, new in
                viewport.visibleRect = new
            }
        } else {
            content
        }
    }
}
