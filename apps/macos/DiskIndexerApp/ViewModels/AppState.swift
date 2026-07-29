import Combine
import Foundation

@MainActor
final class AppState: ObservableObject {
    @Published var databasePath: String
    @Published private(set) var volumes: [VolumeSummary] = []
    @Published private(set) var tasks: [TaskRecord] = []
    @Published private(set) var isLoading = false
    @Published var statusMessage = "准备就绪"
    @Published var errorMessage: String?

    let runner: RustCommandRunner

    init(runner: RustCommandRunner = RustCommandRunner()) {
        self.runner = runner
        let applicationSupport = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first ?? FileManager.default.homeDirectoryForCurrentUser
        databasePath = applicationSupport
            .appendingPathComponent("DiskIndexer", isDirectory: true)
            .appendingPathComponent("index.db")
            .path
    }

    func refresh() async {
        isLoading = true
        defer { isLoading = false }
        do {
            async let loadedVolumes: [VolumeSummary] = runner.runJSON(
                databasePath: databasePath,
                command: ["volume", "list", "--json"]
            )
            async let loadedTasks: [TaskRecord] = runner.runJSON(
                databasePath: databasePath,
                command: ["tasks", "--json"]
            )
            volumes = try await loadedVolumes
            tasks = try await loadedTasks
            errorMessage = nil
            statusMessage = "已刷新 \(volumes.count) 个卷"
        } catch {
            errorMessage = error.localizedDescription
            statusMessage = "无法加载本地数据"
        }
    }

    func addVolume(path: String, role: String) async {
        guard !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            errorMessage = "请选择或输入卷根目录。"
            return
        }
        isLoading = true
        defer { isLoading = false }
        do {
            let response: VolumeAddResponse = try await runner.runJSON(
                databasePath: databasePath,
                command: ["volume", "add", path, "--role", role, "--json"]
            )
            if let conflict = response.identityConflict {
                errorMessage = "检测到可能克隆盘（冲突 #\(conflict.id)）：\(conflict.candidateMountPath)。原卷没有被覆盖，请在“硬盘”页审核。"
            } else {
                statusMessage = "已注册 \(response.volume?.volumeName ?? path)"
            }
            await refresh()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
