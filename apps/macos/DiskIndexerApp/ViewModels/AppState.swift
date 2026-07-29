import AppKit
import Combine
import Foundation

enum ScanMode: String, CaseIterable, Identifiable {
    case metadataOnly
    case incremental
    case fullHash

    var id: String { rawValue }
    var label: String {
        switch self {
        case .metadataOnly: "仅元数据"
        case .incremental: "普通增量扫描"
        case .fullHash: "完整哈希扫描"
        }
    }
}

@MainActor
final class AppState: ObservableObject {
    @Published var databasePath: String
    @Published private(set) var volumes: [VolumeSummary] = []
    @Published private(set) var conflicts: [VolumeConflict] = []
    @Published private(set) var tasks: [TaskRecord] = []
    @Published private(set) var dashboardStats: DashboardStats?
    @Published private(set) var duplicateGroups: [DuplicateGroupModel] = []
    @Published private(set) var duplicateNextCursor: Int?
    @Published private(set) var lookupResult: LookupResultModel?
    @Published private(set) var cleanupPlan: CleanupPlanModel?
    @Published private(set) var logs: [AppLogEntry] = []
    @Published private(set) var isLoading = false
    @Published var statusMessage = "准备就绪"
    @Published var errorMessage: String?

    let runner: RustCommandRunner
    let taskController = TaskProcessController()
    private var cleanupOutputURL: URL?
    private var initializedDatabasePaths: Set<String> = []

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
        taskController.onEvent = { [weak self] event in
            self?.record(.info, operation: event.operation, message: eventDescription(event))
        }
        taskController.onDiagnostic = { [weak self] message in
            self?.record(.warning, operation: "Rust CLI", message: message)
        }
        taskController.onFinish = { [weak self] status in
            guard let self else { return }
            let logLevel: AppLogEntry.Level
            if case .failed = status {
                logLevel = .error
            } else if status == .cancelled {
                logLevel = .warning
            } else {
                logLevel = .info
            }
            self.record(
                logLevel,
                operation: "任务",
                message: "本地任务\(status.label)"
            )
            if case .failed = status {
                self.errorMessage = "任务失败：\(status.label)。详见“日志”页。"
            }
            if status == .completed, let output = self.cleanupOutputURL {
                self.loadCleanupPlan(at: output)
                self.cleanupOutputURL = nil
            }
            Task { await self.refresh() }
        }
    }

    var isTaskRunning: Bool { taskController.status.isRunning }

    func refresh() async {
        isLoading = true
        defer { isLoading = false }
        do {
            if !initializedDatabasePaths.contains(databasePath) {
                _ = try await runner.run(databasePath: databasePath, command: ["init"])
                initializedDatabasePaths.insert(databasePath)
            }
            async let loadedVolumes: [VolumeSummary] = runner.runJSON(
                databasePath: databasePath,
                command: ["volume", "list", "--json"]
            )
            async let loadedTasks: [TaskRecord] = runner.runJSON(
                databasePath: databasePath,
                command: ["tasks", "--json", "--limit", "100"]
            )
            async let loadedConflicts: [VolumeConflict] = runner.runJSON(
                databasePath: databasePath,
                command: ["volume", "conflicts", "--json"]
            )
            async let loadedStats: DashboardStats = runner.runJSON(
                databasePath: databasePath,
                command: ["stats", "--json"]
            )
            volumes = try await loadedVolumes
            tasks = try await loadedTasks
            conflicts = try await loadedConflicts
            dashboardStats = try await loadedStats
            errorMessage = nil
            statusMessage = "已刷新 \(volumes.count) 个卷、\(tasks.count) 条任务"
        } catch {
            errorMessage = error.localizedDescription
            statusMessage = "无法加载本地数据"
            record(.error, operation: "刷新", message: error.localizedDescription)
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
                errorMessage = "检测到可能克隆盘（冲突 #\(conflict.id)）：\(conflict.candidateMountPath)。原卷没有被覆盖；请审核后再明确保留为新卷。"
                record(.warning, operation: "注册卷", message: "检测到 possible_clone 冲突 #\(conflict.id)")
            } else {
                statusMessage = "已注册 \(response.volume?.volumeName ?? path)"
                record(.info, operation: "注册卷", message: statusMessage)
            }
            await refresh()
        } catch {
            errorMessage = error.localizedDescription
            record(.error, operation: "注册卷", message: error.localizedDescription)
        }
    }

    func applyDatabasePath(_ path: String) async {
        guard !isTaskRunning else {
            errorMessage = "请先等待或取消当前任务，再切换数据库。"
            return
        }
        let url = URL(fileURLWithPath: path)
        guard FileManager.default.fileExists(atPath: url.path) else {
            errorMessage = "为了避免静默创建错误数据库，设置页只能切换到已存在的 SQLite 文件。"
            return
        }
        isLoading = true
        defer { isLoading = false }
        do {
            let _: DashboardStats = try await runner.runJSON(databasePath: url.path, command: ["stats", "--json"])
            databasePath = url.path
            record(.info, operation: "设置", message: "已验证并切换数据库 \(url.lastPathComponent)")
            await refresh()
        } catch {
            errorMessage = "无法打开所选数据库：\(error.localizedDescription)"
            record(.error, operation: "设置", message: errorMessage ?? "")
        }
    }

    func resolveConflictAsNewVolume(_ conflict: VolumeConflict, role: String) async {
        isLoading = true
        defer { isLoading = false }
        do {
            _ = try await runner.run(
                databasePath: databasePath,
                command: ["volume", "resolve", "--conflict", "\(conflict.id)", "--as-new-volume", "--role", role, "--json"]
            )
            record(.info, operation: "身份冲突", message: "冲突 #\(conflict.id) 已明确保留为新卷")
            await refresh()
        } catch {
            errorMessage = error.localizedDescription
            record(.error, operation: "身份冲突", message: error.localizedDescription)
        }
    }

    func startScan(volume: VolumeSummary, mode: ScanMode) {
        var command = ["scan", volume.mountPath]
        switch mode {
        case .metadataOnly: command.append("--metadata-only")
        case .incremental: break
        case .fullHash: command.append("--full-hash")
        }
        command.append("--jsonl-progress")
        startLongTask(command, description: "扫描 \(volume.volumeName)")
    }

    func startFullHash(volume: VolumeSummary) {
        startLongTask(
            ["hash", "complete", "--volume", "\(volume.id)", "--jsonl-progress"],
            description: "补齐完整哈希 \(volume.volumeName)"
        )
    }

    func cancelTask() {
        taskController.cancel()
        record(.warning, operation: "任务", message: "已请求安全取消；CLI 将先处理 SIGINT。")
    }

    func loadFirstDuplicatePage() async {
        duplicateGroups = []
        duplicateNextCursor = 0
        await loadMoreDuplicates()
    }

    func loadMoreDuplicates() async {
        guard let cursor = duplicateNextCursor else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            let page: DuplicateGroupsPage = try await runner.runJSON(
                databasePath: databasePath,
                command: [
                    "duplicates", "--json", "--page", "--after-content-id", "\(cursor)", "--limit", "50",
                ]
            )
            duplicateGroups.append(contentsOf: page.groups)
            duplicateNextCursor = page.nextAfterContentID
            statusMessage = "已加载 \(duplicateGroups.count) 个重复组"
        } catch {
            errorMessage = error.localizedDescription
            record(.error, operation: "重复文件", message: error.localizedDescription)
        }
    }

    func lookup(path: String, fullHash: Bool) async {
        guard !path.isEmpty else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            var command = ["lookup", path, "--json"]
            if fullHash { command.append("--full-hash") }
            lookupResult = try await runner.runJSON(databasePath: databasePath, command: command)
            if lookupResult?.cacheState == "cache_stale" {
                record(.warning, operation: "文件查询", message: "索引记录已过期，需要重新计算后才能精确判断。")
            }
        } catch {
            errorMessage = error.localizedDescription
            record(.error, operation: "文件查询", message: error.localizedDescription)
        }
    }

    func generateCleanupPlan(
        targetVolumeID: Int,
        keepVolumeID: Int,
        minimumCopies: Int,
        minimumDevices: Int,
        verifyMetadata: Bool,
        verifyFullHash: Bool,
        outputURL: URL
    ) {
        cleanupOutputURL = outputURL
        var command = [
            "cleanup", "plan", "--target-volume", "\(targetVolumeID)", "--keep-volume", "\(keepVolumeID)",
            "--min-remaining-copies", "\(minimumCopies)",
            "--min-remaining-physical-devices", "\(minimumDevices)", "--output", outputURL.path,
        ]
        if verifyMetadata { command.append("--verify-metadata") }
        if verifyFullHash { command.append("--verify-full-hash") }
        command.append("--jsonl-progress")
        startLongTask(command, description: "生成只读清理计划")
    }

    func exportLogs(to url: URL) throws {
        let body = logs.map { entry in
            "\(ISO8601DateFormatter().string(from: entry.timestamp))\t\(entry.level.rawValue)\t\(entry.operation)\t\(entry.message)"
        }.joined(separator: "\n")
        try body.write(to: url, atomically: true, encoding: .utf8)
    }

    func showCopyInFinder(_ copy: CopyViewModel) {
        guard let volume = volumes.first(where: { $0.id == copy.volumeID }) else {
            errorMessage = "无法定位副本所属卷，不能在 Finder 中显示。"
            return
        }
        NSWorkspace.shared.activateFileViewerSelecting([
            URL(fileURLWithPath: volume.mountPath).appendingPathComponent(copy.path),
        ])
    }

    private func startLongTask(_ command: [String], description: String) {
        do {
            try taskController.start(runner: runner, databasePath: databasePath, command: command)
            statusMessage = "已开始\(description)"
            record(.info, operation: "任务", message: statusMessage)
        } catch {
            errorMessage = error.localizedDescription
            record(.error, operation: "任务", message: error.localizedDescription)
        }
    }

    private func loadCleanupPlan(at url: URL) {
        do {
            let data = try Data(contentsOf: url)
            cleanupPlan = try JSONDecoder().decode(CleanupPlanModel.self, from: data)
            statusMessage = "清理计划已生成：\(cleanupPlan?.items.count ?? 0) 条候选"
            record(.info, operation: "清理计划", message: "已写入 \(url.lastPathComponent)")
        } catch {
            errorMessage = "清理计划已结束，但无法读取导出文件：\(error.localizedDescription)"
            record(.error, operation: "清理计划", message: errorMessage ?? "")
        }
    }

    private func record(_ level: AppLogEntry.Level, operation: String, message: String) {
        logs.append(AppLogEntry(level: level, operation: operation, message: message))
        if logs.count > 500 { logs.removeFirst(logs.count - 500) }
    }
}

private func eventDescription(_ event: JSONLTaskEvent) -> String {
    switch event.type {
    case "task_started": "\(event.operation) 已开始"
    case "task_completed": "\(event.operation) \(event.status ?? "结束")"
    case "progress":
        switch event.operation {
        case "hash_complete": "完整哈希：已处理 \(event.filesProcessed.map(String.init) ?? "—") 个文件"
        case "verify": "验证：已通过 \(event.filesVerified.map(String.init) ?? "—") 个文件"
        case "cleanup_plan": "清理计划：已处理 \(event.groupsProcessed.map(String.init) ?? "—") 个重复组"
        default: "扫描：已发现 \(event.filesSeen.map(String.init) ?? "—") 个文件"
        }
    default: "\(event.operation)：\(event.type)"
    }
}
