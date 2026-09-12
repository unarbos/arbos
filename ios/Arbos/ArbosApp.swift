import SwiftUI

@main
struct ArbosApp: App {
    @StateObject private var settings = AppSettings()

    var body: some Scene {
        WindowGroup {
            CallView(settings: settings)
                .environmentObject(settings)
        }
    }
}
