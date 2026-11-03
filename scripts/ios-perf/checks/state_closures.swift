import SwiftUI
struct S: View {
    let model: Int
    let chatId: String
    @Environment(\.dismiss) private var dismiss
    @State private var rows: [Int] = []
    @StateObject private var rec = Rec()
    @FocusState private var f: Bool
    var body: some View { Text("x") }
}
final class Rec: ObservableObject {}
