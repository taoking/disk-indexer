import SwiftUI

@main
struct DiskIndexerApp: App {
    @StateObject private var appState = AppState()

    var body: some Scene {
        WindowGroup("Disk Indexer") {
            RootView()
                .environmentObject(appState)
                .frame(minWidth: 920, minHeight: 620)
        }
        .defaultSize(width: 1120, height: 760)
    }
}
