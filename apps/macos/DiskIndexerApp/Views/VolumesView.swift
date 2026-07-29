import AppKit
import SwiftUI

struct VolumesView: View {
    @EnvironmentObject private var appState: AppState
    @State private var path = ""
    @State private var role = "unknown"

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("硬盘").font(.largeTitle.bold())
                Spacer()
                Button("刷新") { Task { await appState.refresh() } }
            }
            GroupBox("注册硬盘") {
                HStack {
                    TextField("卷根目录", text: $path)
                    Button("选择…") { chooseDirectory() }
                    Picker("角色", selection: $role) {
                        Text("未知").tag("unknown")
                        Text("主存储").tag("primary")
                        Text("本地备份").tag("local_backup")
                        Text("异地备份").tag("offsite_backup")
                        Text("旧备份").tag("legacy_backup")
                    }
                    .frame(width: 140)
                    Button("注册") { Task { await appState.addVolume(path: path, role: role) } }
                        .disabled(appState.isLoading)
                }
                Text("若 marker 可能来自克隆盘，CLI 会返回 possible_clone，应用不会覆盖或扫描历史卷。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Table(appState.volumes) {
                TableColumn("名称") { Text($0.volumeName) }
                TableColumn("挂载路径") { Text($0.mountPath).lineLimit(1) }
                TableColumn("角色") { Text($0.role) }
                TableColumn("状态") { Text($0.isOnline ? "在线" : "离线") }
                TableColumn("身份") { Text($0.identityState) }
                TableColumn("System UUID") { Text($0.systemVolumeUUID ?? "—").lineLimit(1) }
                TableColumn("物理设备") { Text($0.physicalDeviceID.map(String.init) ?? "目录索引") }
                TableColumn("操作") { volume in
                    HStack {
                        Button("扫描") { appState.startScan(volume: volume, mode: .incremental) }
                            .disabled(!volume.isOnline || appState.isTaskRunning)
                        Button("在 Finder 中显示") {
                            NSWorkspace.shared.open(URL(fileURLWithPath: volume.mountPath))
                        }
                    }
                }
            }
            if !appState.conflicts.isEmpty {
                GroupBox("待人工确认的身份冲突") {
                    ForEach(appState.conflicts) { conflict in
                        HStack {
                            Image(systemName: "exclamationmark.triangle")
                                .foregroundStyle(.orange)
                            Text("#\(conflict.id) · \(conflict.candidateMountPath)").lineLimit(1)
                            Spacer()
                            Button("明确保留为新卷") {
                                Task { await appState.resolveConflictAsNewVolume(conflict, role: "unknown") }
                            }
                            .disabled(appState.isLoading)
                        }
                    }
                    Text("此操作不会合并或覆盖原卷，且会写入本地审计事件。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(24)
    }

    private func chooseDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        if panel.runModal() == .OK, let url = panel.url {
            path = url.path
        }
    }
}
