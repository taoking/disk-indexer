import SwiftUI

enum AppSection: String, CaseIterable, Identifiable {
    case overview = "概览"
    case volumes = "硬盘"
    case tasks = "扫描任务"
    case duplicates = "重复文件"
    case lookup = "文件查询"
    case cleanup = "清理计划"
    case settings = "设置"
    case logs = "日志"

    var id: String { rawValue }

    var systemImage: String {
        switch self {
        case .overview: "rectangle.3.group"
        case .volumes: "externaldrive"
        case .tasks: "arrow.triangle.2.circlepath"
        case .duplicates: "square.on.square"
        case .lookup: "doc.text.magnifyingglass"
        case .cleanup: "checklist"
        case .settings: "gearshape"
        case .logs: "text.justify.leading"
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
            case .tasks:
                TasksView()
            case .duplicates:
                DuplicatesView()
            case .lookup:
                LookupView()
            case .cleanup:
                CleanupView()
            case .settings:
                SettingsView()
            case .logs:
                LogsView()
            }
        }
        .task {
            await refreshOnLaunch()
        }
        .alert("本地 CLI 错误", isPresented: errorBinding) {
            Button("好", role: .cancel) { appState.errorMessage = nil }
        } message: {
            Text(appState.errorMessage ?? "未知错误")
        }
    }

    @EnvironmentObject private var appState: AppState

    private func refreshOnLaunch() async {
        await appState.refresh()
    }

    private var errorBinding: Binding<Bool> {
        Binding(get: { appState.errorMessage != nil }, set: { if !$0 { appState.errorMessage = nil } })
    }
}
