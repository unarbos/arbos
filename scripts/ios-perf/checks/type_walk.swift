import SwiftUI
struct Leaf: View { @State var n = 0; var body: some View { Text("\(n)") } }
struct Deep: View { var body: some View { HStack { Leaf(); Image(systemName: "x") } } }
struct Mod: ViewModifier { func body(content: Content) -> some View { content.overlay { Deep() } } }
struct Root: View {
    @State var flag = false
    let items = [1, 2]
    var body: some View {
        ScrollViewReader { _ in
            ScrollView {
                LazyVStack {
                    if flag { Leaf() } else { Text("no").background(Deep()) }
                    ForEach(items, id: \.self) { _ in Leaf().modifier(Mod()) }
                    Text("a"); Text("b"); Color.red
                }
            }
        }.overlay(alignment: .top) { Button("x") {} }
    }
}
var w = TypeWalk()
w.visit(Root.self)
print("visited", w.visited)
print(TypeWalk.tupleElements((Int, String, Leaf).self))
print(TypeWalk.tupleElements(Int.self))

/// Resolves a view type's body type, and the bodies of everything inside it, the way SwiftUI
/// does when it first builds the view, and asks each type the conformance questions SwiftUI
/// asks of it. Children SwiftUI keeps as generic parameters (`ModifiedContent`, `if`/`else`,
/// `ForEach`, stacks, tuples) are followed; type-erased (`AnyView`) and closure-built content
/// is not.
struct TypeWalk {
    private var seen = Set<ObjectIdentifier>()
    static let limit = 6000
    var visited: Int { seen.count }

    mutating func check(_ type: Any.Type) {
        _ = type is DynamicProperty.Type
        _ = type is any Equatable.Type
    }

    mutating func visit(_ type: Any.Type) {
        guard seen.count < Self.limit, seen.insert(ObjectIdentifier(type)).inserted else { return }
        print(String(describing: type).prefix(110))
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

extension ModifiedContent: WarmContainer { static var warmChildren: [Any.Type] { [Content.self, Modifier.self] } }
extension _ConditionalContent: WarmContainer { static var warmChildren: [Any.Type] { [TrueContent.self, FalseContent.self] } }
extension Optional: WarmContainer { static var warmChildren: [Any.Type] { [Wrapped.self] } }
extension ForEach: WarmContainer { static var warmChildren: [Any.Type] { [Content.self] } }
extension TupleView: WarmContainer { static var warmChildren: [Any.Type] { TypeWalk.tupleElements(T.self) } }
extension Group: WarmContainer { static var warmChildren: [Any.Type] { [Content.self] } }
extension VStack: WarmContainer { static var warmChildren: [Any.Type] { [Content.self] } }
extension HStack: WarmContainer { static var warmChildren: [Any.Type] { [Content.self] } }
extension ZStack: WarmContainer { static var warmChildren: [Any.Type] { [Content.self] } }
extension LazyVStack: WarmContainer { static var warmChildren: [Any.Type] { [Content.self] } }
extension LazyHStack: WarmContainer { static var warmChildren: [Any.Type] { [Content.self] } }
extension ScrollView: WarmContainer { static var warmChildren: [Any.Type] { [Content.self] } }
extension ScrollViewReader: WarmContainer { static var warmChildren: [Any.Type] { [Content.self] } }
extension GeometryReader: WarmContainer { static var warmChildren: [Any.Type] { [Content.self] } }
extension Button: WarmContainer { static var warmChildren: [Any.Type] { [Label.self] } }
extension _OverlayModifier: WarmContainer { static var warmChildren: [Any.Type] { [Overlay.self] } }
extension _BackgroundModifier: WarmContainer { static var warmChildren: [Any.Type] { [Background.self] } }
