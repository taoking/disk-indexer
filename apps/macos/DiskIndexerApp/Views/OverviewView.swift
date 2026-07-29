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
                    MetricCard(title: "已注册硬盘", value: "\(appState.volumes.count)", icon: "externaldrive")
                    MetricCard(
                        title: "在线硬盘",
                        value: "\(appState.volumes.filter(\.isOnline).count)",
                        icon: "checkmark.circle"
                    )
                    MetricCard(title: "最近任务", value: "\(appState.tasks.count)", icon: "clock")
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
            }
            .padding(24)
        }
        .alert("本地 CLI 错误", isPresented: errorBinding) {
            Button("好", role: .cancel) { appState.errorMessage = nil }
        } message: {
            Text(appState.errorMessage ?? "未知错误")
        }
    }

    private var errorBinding: Binding<Bool> {
        Binding(get: { appState.errorMessage != nil }, set: { if !$0 { appState.errorMessage = nil } })
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
