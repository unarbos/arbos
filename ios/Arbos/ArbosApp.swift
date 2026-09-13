import SwiftUI

@main
struct ArbosApp: App {
    @StateObject private var settings: AppSettings
    @StateObject private var link: VoiceLink
    @StateObject private var chat: ChatStore

    init() {
        let settings = AppSettings()
        let link = VoiceLink(settings: settings)
        _settings = StateObject(wrappedValue: settings)
        _link = StateObject(wrappedValue: link)
        _chat = StateObject(wrappedValue: ChatStore(settings: settings, link: link))
    }

    var body: some Scene {
        WindowGroup {
            CallView(settings: settings, chat: chat, link: link)
                .environmentObject(settings)
                .environmentObject(chat)
                .environmentObject(link)
        }
    }
}
