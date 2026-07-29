import SwiftUI

struct OverviewView: View {
    @EnvironmentObject private var appState: AppState

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                Text("概览")
                    .font(.largeTitle.bold())
                Text("所有操作在本机通过内置 Rust CLI 完成；此应用不会启动浏览器、HTTP 服务或监听端口。")
                    .foregroundStyle(.secondary)
                HStack(spacing: 16) {
                    MetricCard(title: "已注册硬盘", value: "\(appState.dashboardStats?.registeredVolumes ?? appState.volumes.count)", icon: "externaldrive")
                    MetricCard(
                        title: "在线硬盘",
                        value: "\(appState.dashboardStats?.onlineVolumes ?? appState.volumes.filter(\.isOnline).count)",
                        icon: "checkmark.circle"
                    )
                    MetricCard(title: "已索引文件", value: "\(appState.dashboardStats?.indexedFiles ?? 0)", icon: "doc")
                    MetricCard(title: "可信重复组", value: "\(appState.dashboardStats?.trustedDuplicateGroups ?? 0)", icon: "square.on.square")
                }
                if let stats = appState.dashboardStats {
                    GroupBox("索引统计") {
                        Grid(alignment: .leading, horizontalSpacing: 32, verticalSpacing: 8) {
                            GridRow { Text("数据库 schema"); Text("v\(stats.schemaVersion)") }
                            GridRow { Text("离线卷"); Text("\(stats.offlineVolumes)") }
                            GridRow { Text("完整哈希文件"); Text("\(stats.fullHashedFiles)") }
                            GridRow { Text("可验证在线副本"); Text("\(stats.verifiableOnlineCopies)") }
                            GridRow { Text("理论可释放空间"); Text(ByteCountFormatter.string(fromByteCount: stats.theoreticalReclaimableBytes, countStyle: .file)) }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                GroupBox("最近任务") {
                    if appState.tasks.isEmpty {
                        ContentUnavailableView("尚无任务", systemImage: "clock", description: Text("扫描、验证和生成清理计划后会显示在这里。"))
                    } else {
                        ForEach(appState.tasks.prefix(5)) { task in
                            HStack {
                                Text(task.operation)
                                Spacer()
                                Text(task.status).foregroundStyle(.secondary)
                            }
                            .padding(.vertical, 3)
                        }
                    }
                }
                HStack {
                    Button("刷新") { Task { await appState.refresh() } }
                    if appState.isLoading { ProgressView() }
                    Text(appState.statusMessage).foregroundStyle(.secondary)
                }
                Text("理论可释放空间不是删除建议；内容重复不等于副本多余。")
                    .foregroundStyle(.orange)
            }
            .padding(24)
        }
    }
}

private struct MetricCard: View {
    let title: String
    let value: String
    let icon: String

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(title, systemImage: icon).foregroundStyle(.secondary)
            Text(value).font(.title.bold())
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding()
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 12))
    }
}
