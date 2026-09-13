import SwiftUI

@main
struct ArbosApp: App {
    @StateObject private var settings: AppSettings
    @StateObject private var chat: ChatStore

    init() {
        let settings = AppSettings()
        _settings = StateObject(wrappedValue: settings)
        _chat = StateObject(wrappedValue: ChatStore(settings: settings))
    }

    var body: some Scene {
        WindowGroup {
            CallView(settings: settings, chat: chat)
                .environmentObject(settings)
                .environmentObject(chat)
        }
    }
}
