import AppKit
import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var appState: AppState

    var body: some View {
        Form {
            Section("数据库") {
                TextField("SQLite 数据库路径", text: $appState.databasePath)
                HStack {
                    Button("选择位置…") { chooseDatabaseLocation() }
                    Button("验证并刷新") { Task { await appState.refresh() } }
                }
                Text("每条 Rust 命令都会显式传入此路径；应用不使用 PATH，也不直接写入 SQLite 业务表。")
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
    }

    private func chooseDatabaseLocation() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "index.db"
        if panel.runModal() == .OK, let url = panel.url {
            appState.databasePath = url.path
        }
    }
}
