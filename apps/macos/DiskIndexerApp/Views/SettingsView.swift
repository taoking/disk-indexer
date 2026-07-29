import AppKit
import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var appState: AppState
    @State private var pendingDatabasePath = ""

    var body: some View {
        Form {
            Section("数据库") {
                TextField("SQLite 数据库路径", text: $pendingDatabasePath)
                HStack {
                    Button("选择位置…") { chooseDatabaseLocation() }
                    Button("验证并切换") { Task { await appState.applyDatabasePath(pendingDatabasePath) } }
                        .disabled(appState.isTaskRunning || pendingDatabasePath == appState.databasePath)
                }
                Text("切换时会先阻止运行任务，并只接受已存在、可通过 Rust CLI 打开的 SQLite 文件，避免静默创建错误数据库。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section("安全边界") {
                Label("当前版本只生成清理计划，不会删除、移动或修改任何原始文件。", systemImage: "lock.shield")
                Label("应用通过 Process.arguments 调用 Bundle 内置 CLI，不使用 shell、HTTP 或端口。", systemImage: "terminal")
            }
        }
        .formStyle(.grouped)
        .padding(24)
        .navigationTitle("设置")
        .onAppear { pendingDatabasePath = appState.databasePath }
    }

    private func chooseDatabaseLocation() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        if panel.runModal() == .OK, let url = panel.url {
            pendingDatabasePath = url.path
        }
    }
}
