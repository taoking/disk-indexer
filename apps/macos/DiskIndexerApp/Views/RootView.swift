import SwiftUI

enum AppSection: String, CaseIterable, Identifiable {
    case overview = "概览"
    case volumes = "硬盘"
    case settings = "设置"

    var id: String { rawValue }

    var systemImage: String {
        switch self {
        case .overview: "rectangle.3.group"
        case .volumes: "externaldrive"
        case .settings: "gearshape"
        }
    }
}

struct RootView: View {
    @State private var selection: AppSection? = .overview

    var body: some View {
        NavigationSplitView {
            List(AppSection.allCases, selection: $selection) { section in
                Label(section.rawValue, systemImage: section.systemImage)
                    .tag(section)
            }
            .navigationTitle("Disk Indexer")
        } detail: {
            switch selection ?? .overview {
            case .overview:
                OverviewView()
            case .volumes:
                VolumesView()
            case .settings:
                SettingsView()
            }
        }
        .task {
            await refreshOnLaunch()
        }
    }

    @EnvironmentObject private var appState: AppState

    private func refreshOnLaunch() async {
        await appState.refresh()
    }
}
