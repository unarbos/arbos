import Metal
import os
import SwiftUI

/// The first chat open after a launch paid about 260 ms of main-thread work that no later
/// open repeats, whichever chat it was: Swift conformance scans and type-metadata lookups for
/// the chat screen's view types, SwiftUI reflecting `ChatSession`'s fields, the first
/// `AttributedString` → `NSAttributedString` conversion, and the first Metal device
/// (ios-perf-open-breakdown-and-baseline). Every one of those is a process-wide cache in the
/// Swift runtime, Foundation or Metal, so filling them on a background thread after the first
/// frame takes them out of the first tap without drawing anything.
///
/// Nothing here builds a view graph: rendering the chat off screen would run its appear hooks
/// (mark-read, `visibleChatId`, the jobs poll) against the real model.
/// `ARBOS_NO_WARMUP=1` turns it off, for measuring the difference.
enum FirstOpenWarmup {
    private static let signposter = OSSignposter(subsystem: "com.unarbos.arbos.ios", category: "launch")

    @MainActor
    static func run(model: AppModel) async {
        guard ProcessInfo.processInfo.environment["ARBOS_NO_WARMUP"] != "1" else { return }
        // Well after the chat list's first frame (committed in the turn the root appears).
        try? await Task.sleep(for: .milliseconds(500))
        // The value only carries `ChatSession`'s stored-property types to the reflection
        // below; it never enters a view graph, so none of its hooks run.
        let session = TypeBox(ChatSession(model: model, chatId: "", projectId: "", tick: 0))
        await Task.detached(priority: .utility) {
            let state = signposter.beginInterval("first-open-warmup")
            _ = MTLCreateSystemDefaultDevice()
            _ = try? NSAttributedString(AttributedString("warm"), including: AttributeScopes.SwiftUIAttributes.self)
            var walk = TypeWalk()
            for child in Mirror(reflecting: session.value).children { walk.check(type(of: child.value)) }
            for root in roots { walk.visit(root) }
            signposter.endInterval("first-open-warmup", state, "\(walk.visited) types")
        }.value
    }

    private static let roots: [Any.Type] = [ChatView.self, ChatSession.self, RowView.self]
}

private struct TypeBox: @unchecked Sendable {
    let value: Any
    init(_ value: Any) { self.value = value }
}

/// Resolves a view type's body type, and the bodies of everything inside it, the way SwiftUI
/// does when it first builds the view, and asks each type the conformance questions SwiftUI
/// asks of it. Children SwiftUI keeps as generic parameters (`ModifiedContent`, `if`/`else`,
/// `ForEach`, stacks, tuples) are followed; type-erased (`AnyView`) and closure-built content
/// is not.
private struct TypeWalk {
    private var seen = Set<ObjectIdentifier>()
    private static let limit = 6000
    var visited: Int { seen.count }

    func check(_ type: Any.Type) {
        _ = type is DynamicProperty.Type
        _ = type is any Equatable.Type
    }

    mutating func visit(_ type: Any.Type) {
        guard seen.count < Self.limit, seen.insert(ObjectIdentifier(type)).inserted else { return }
        check(type)
        if let container = type as? WarmContainer.Type {
            for child in container.warmChildren { visit(child) }
        }
        if let view = type as? any View.Type {
            let body = Self.body(of: view)
            if body != Never.self { visit(body) }
        }
        if let modifier = type as? any ViewModifier.Type {
            let body = Self.body(of: modifier)
            if body != Never.self { visit(body) }
        }
    }

    private static func body<V: View>(of _: V.Type) -> Any.Type { V.Body.self }
    private static func body<M: ViewModifier>(of _: M.Type) -> Any.Type { M.Body.self }

    /// A tuple's element types (`TupleView<(A, B, C)>`), read from its type metadata, whose
    /// layout is part of the stable Swift ABI: kind (0x301 for a tuple), element count,
    /// labels, then (type, offset) pairs.
    static func tupleElements(_ type: Any.Type) -> [Any.Type] {
        let metadata = unsafeBitCast(type, to: UnsafeRawPointer.self)
        guard metadata.load(as: UInt.self) == 0x301 else { return [] }
        let count = metadata.load(fromByteOffset: 8, as: Int.self)
        guard (1...64).contains(count) else { return [] }
        return (0..<count).map { i in
            unsafeBitCast(metadata.load(fromByteOffset: 24 + 16 * i, as: UnsafeRawPointer.self), to: Any.Type.self)
        }
    }
}

private protocol WarmContainer {
    static var warmChildren: [Any.Type] { get }
}

extension ModifiedContent: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Content.self, Modifier.self] } }
extension _ConditionalContent: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [TrueContent.self, FalseContent.self] } }
extension Optional: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Wrapped.self] } }
extension ForEach: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Content.self] } }
extension TupleView: WarmContainer { fileprivate static var warmChildren: [Any.Type] { TypeWalk.tupleElements(T.self) } }
extension Group: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Content.self] } }
extension VStack: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Content.self] } }
extension HStack: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Content.self] } }
extension ZStack: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Content.self] } }
extension LazyVStack: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Content.self] } }
extension LazyHStack: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Content.self] } }
extension ScrollView: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Content.self] } }
extension ScrollViewReader: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Content.self] } }
extension GeometryReader: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Content.self] } }
extension Button: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Label.self] } }
extension _OverlayModifier: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Overlay.self] } }
extension _BackgroundModifier: WarmContainer { fileprivate static var warmChildren: [Any.Type] { [Background.self] } }
