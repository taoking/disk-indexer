import AppKit
import SwiftUI

struct TasksView: View {
    @EnvironmentObject private var appState: AppState
    @State private var selectedVolumeID: Int?
    @State private var scanMode: ScanMode = .incremental

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("扫描任务").font(.largeTitle.bold())
                Spacer()
                Text(appState.taskController.status.label)
                    .foregroundStyle(appState.isTaskRunning ? .orange : .secondary)
                if appState.isTaskRunning {
                    Button("取消任务", role: .destructive) { appState.cancelTask() }
                }
            }
            GroupBox("开始扫描") {
                HStack {
                    Picker("硬盘", selection: $selectedVolumeID) {
                        Text("选择硬盘").tag(Optional<Int>.none)
                        ForEach(appState.volumes) { volume in
                            Text("\(volume.volumeName)（\(volume.isOnline ? "在线" : "离线")）")
                                .tag(Optional(volume.id))
                        }
                    }
                    .frame(width: 300)
                    Picker("模式", selection: $scanMode) {
                        ForEach(ScanMode.allCases) { mode in Text(mode.label).tag(mode) }
                    }
                    .frame(width: 160)
                    Button("开始扫描") {
                        if let volume = appState.volumes.first(where: { $0.id == selectedVolumeID }) {
                            appState.startScan(volume: volume, mode: scanMode)
                        }
                    }
                    .disabled(selectedVolumeID == nil || appState.isTaskRunning)
                    Button("补齐完整哈希") {
                        if let volume = appState.volumes.first(where: { $0.id == selectedVolumeID }) {
                            appState.startFullHash(volume: volume)
                        }
                    }
                    .disabled(selectedVolumeID == nil || appState.isTaskRunning)
                }
            }
            if let event = appState.taskController.currentEvent {
                GroupBox("实时进度") {
                        Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 8) {
                            GridRow { Text("操作"); Text(event.operation) }
                        if event.operation == "scan" {
                            GridRow { Text("已发现 / 已处理"); Text("\(progressValue(event.filesSeen)) / \(progressValue(event.filesProcessed))") }
                            GridRow { Text("元数据复用"); Text(progressValue(event.filesReused)) }
                            GridRow { Text("抽样 / 完整哈希"); Text("\(progressValue(event.filesSampled)) / \(progressValue(event.filesFullHashed))") }
                        } else if event.operation == "hash_complete" {
                            GridRow { Text("已处理"); Text(progressValue(event.filesProcessed)) }
                            GridRow { Text("完整哈希 / 跳过"); Text("\(progressValue(event.filesFullHashed)) / \(progressValue(event.filesSkipped))") }
                            GridRow { Text("失败"); Text(progressValue(event.filesFailed)) }
                        } else if event.operation == "verify" {
                            GridRow { Text("已处理"); Text(progressValue(event.filesProcessed)) }
                            GridRow { Text("已验证 / 失败"); Text("\(progressValue(event.filesVerified)) / \(progressValue(event.filesFailed))") }
                        } else if event.operation == "cleanup_plan" {
                            GridRow { Text("重复组进度"); Text("\(progressValue(event.groupsProcessed)) / \(progressValue(event.groupsSeen))") }
                            GridRow { Text("已验证 / 失败副本"); Text("\(progressValue(event.filesVerified)) / \(progressValue(event.filesFailed))") }
                            if let hash = event.currentGroupHash { GridRow { Text("当前内容组"); Text(hash).font(.caption.monospaced()) } }
                        }
                        GridRow { Text("已读取字节"); Text(byteCountValue(event.bytesRead)) }
                        if let path = event.currentPath {
                            GridRow { Text("当前路径"); Text(path).lineLimit(1) }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            GroupBox("历史任务") {
                Table(appState.tasks) {
                    TableColumn("操作") { Text($0.operation) }
                    TableColumn("状态") { Text($0.status) }
                    TableColumn("开始时间") { Text($0.startedAt) }
                    TableColumn("结束时间") { Text($0.finishedAt ?? "—") }
                }
                .frame(minHeight: 220)
            }
        }
        .padding(24)
        .task {
            await appState.refresh()
            if selectedVolumeID == nil { selectedVolumeID = appState.volumes.first?.id }
        }
    }
}

private func progressValue(_ value: UInt64?) -> String {
    value.map(String.init) ?? "—"
}

private func byteCountValue(_ value: UInt64?) -> String {
    guard let value else { return "—" }
    return ByteCountFormatter.string(fromByteCount: Int64(value), countStyle: .file)
}

private enum DuplicateSort: String, CaseIterable, Identifiable {
    case reclaimable = "理论可释放空间"
    case size = "文件大小"
    case copies = "副本数"
    var id: String { rawValue }
}

struct DuplicatesView: View {
    @EnvironmentObject private var appState: AppState
    @State private var searchText = ""
    @State private var sort = DuplicateSort.reclaimable

    private var groups: [DuplicateGroupModel] {
        let filtered = appState.duplicateGroups.filter { group in
            searchText.isEmpty || group.copies.contains { $0.path.localizedCaseInsensitiveContains(searchText) }
        }
        return filtered.sorted {
            switch sort {
            case .reclaimable: $0.theoreticalReclaimableBytes > $1.theoreticalReclaimableBytes
            case .size: $0.fileSize > $1.fileSize
            case .copies: $0.pathCount > $1.pathCount
            }
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("重复文件").font(.largeTitle.bold())
                Spacer()
                Picker("排序", selection: $sort) {
                    ForEach(DuplicateSort.allCases) { Text($0.rawValue).tag($0) }
                }
                Button("重新加载") { Task { await appState.loadFirstDuplicatePage() } }
            }
            TextField("按已加载路径搜索", text: $searchText)
            Text("按内容 ID 分页读取，每页 50 组；当前排序仅作用于已加载页面，避免将完整报告载入内存。")
                .font(.caption)
                .foregroundStyle(.secondary)
            List(groups) { group in
                DisclosureGroup {
                    ForEach(group.copies) { copy in
                        HStack {
                            Image(systemName: copyStatusIcon(copy.status))
                            Text(copy.path).lineLimit(1)
                            Spacer()
                            Text("\(copy.volume) · \(copy.status)").foregroundStyle(.secondary)
                            Button("在 Finder 中显示") { appState.showCopyInFinder(copy) }
                        }
                    }
                } label: {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(ByteCountFormatter.string(fromByteCount: Int64(group.fileSize), countStyle: .file))
                            .font(.headline)
                        Text("路径 \(group.pathCount) · 存储对象 \(group.storageObjectCount) · 卷 \(group.logicalVolumeCount) · 物理设备 \(group.physicalDeviceCount)")
                        Text("在线可信 \(group.onlineCopies) · 离线历史 \(group.offlineCopies) · 理论 \(ByteCountFormatter.string(fromByteCount: Int64(group.theoreticalReclaimableBytes), countStyle: .file))")
                            .foregroundStyle(.secondary)
                        Text(group.fullHash).font(.caption.monospaced()).foregroundStyle(.secondary)
                    }
                }
            }
            HStack {
                if appState.isLoading { ProgressView() }
                Spacer()
                Button("加载下一页") { Task { await appState.loadMoreDuplicates() } }
                    .disabled(appState.duplicateNextCursor == nil || appState.isLoading)
            }
            Text("理论可释放空间不是删除建议；内容重复不等于副本多余。")
                .foregroundStyle(.orange)
        }
        .padding(24)
        .task {
            if appState.duplicateGroups.isEmpty { await appState.loadFirstDuplicatePage() }
        }
    }
}

struct LookupView: View {
    @EnvironmentObject private var appState: AppState
    @State private var path = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("文件查询").font(.largeTitle.bold())
            HStack {
                TextField("选择一个普通文件", text: $path)
                Button("选择…") { chooseFile() }
                Button("快速查询") { Task { await appState.lookup(path: path, fullHash: false) } }
                    .disabled(path.isEmpty || appState.isLoading)
                Button("重新计算完整哈希") { Task { await appState.lookup(path: path, fullHash: true) } }
                    .disabled(path.isEmpty || appState.isLoading)
            }
            if let result = appState.lookupResult {
                GroupBox("查询结果") {
                    Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 8) {
                        GridRow { Text("路径"); Text(result.path).lineLimit(1) }
                        GridRow { Text("大小"); Text(ByteCountFormatter.string(fromByteCount: Int64(result.fileSize), countStyle: .file)) }
                        GridRow { Text("缓存状态"); Text(result.cacheState) }
                        GridRow { Text("元数据与索引一致"); Text(result.metadataMatchesIndex ? "是" : "否") }
                        GridRow { Text("精确结论"); Text(result.exact ? "是" : "否") }
                        GridRow { Text("已知 / 在线 / 离线副本"); Text("\(result.knownCopies) / \(result.onlineCopies) / \(result.offlineCopies)") }
                    }
                    if result.cacheState == "cache_stale" {
                        Label("索引记录已过期，需要重新计算后才能精确判断", systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                            .padding(.top, 8)
                    }
                    ForEach(result.warnings, id: \.self) { Text($0).foregroundStyle(.secondary) }
                }
            }
        }
        .padding(24)
    }

    private func chooseFile() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        if panel.runModal() == .OK, let url = panel.url { path = url.path }
    }
}

struct CleanupView: View {
    @EnvironmentObject private var appState: AppState
    @State private var targetVolumeID: Int?
    @State private var keepVolumeID: Int?
    @State private var minimumCopies = 2
    @State private var minimumDevices = 1
    @State private var verifyMetadata = false
    @State private var verifyFullHash = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("清理计划").font(.largeTitle.bold())
            Label("当前版本不会删除、移动或修改任何原始文件。", systemImage: "lock.shield")
                .foregroundStyle(.orange)
            GroupBox("生成只读计划") {
                Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 10) {
                    GridRow {
                        Text("目标卷")
                        volumePicker(selection: $targetVolumeID)
                    }
                    GridRow {
                        Text("保留卷")
                        volumePicker(selection: $keepVolumeID)
                    }
                    GridRow {
                        Text("最少剩余副本")
                        Stepper("\(minimumCopies)", value: $minimumCopies, in: 1...10)
                    }
                    GridRow {
                        Text("最少剩余物理设备")
                        Stepper("\(minimumDevices)", value: $minimumDevices, in: 1...10)
                    }
                    GridRow {
                        Text("验证")
                        Toggle("元数据验证", isOn: $verifyMetadata)
                        Toggle("完整哈希验证", isOn: $verifyFullHash)
                    }
                }
                HStack {
                    Spacer()
                    Button("生成并导出 JSON…") { chooseOutputAndGenerate() }
                        .disabled(targetVolumeID == nil || keepVolumeID == nil || targetVolumeID == keepVolumeID || appState.isTaskRunning)
                }
            }
            if let plan = appState.cleanupPlan {
                GroupBox("最近生成的计划（\(plan.items.count) 条）") {
                    List(plan.items) { item in
                        VStack(alignment: .leading) {
                            Text("\(item.candidateDelete.path) · \(item.status)")
                            Text("剩余物理设备 \(item.remainingPhysicalDevices) · \(item.verificationMode)")
                                .foregroundStyle(.secondary)
                            if !item.blockedReasons.isEmpty {
                                Text(item.blockedReasons.joined(separator: "；")).foregroundStyle(.orange)
                            }
                        }
                    }
                }
            }
        }
        .padding(24)
        .onAppear {
            targetVolumeID = targetVolumeID ?? appState.volumes.first?.id
            keepVolumeID = keepVolumeID ?? appState.volumes.dropFirst().first?.id
        }
    }

    @ViewBuilder
    private func volumePicker(selection: Binding<Int?>) -> some View {
        Picker("", selection: selection) {
            Text("选择卷").tag(Optional<Int>.none)
            ForEach(appState.volumes) { volume in
                Text(volume.volumeName).tag(Optional(volume.id))
            }
        }
        .labelsHidden()
        .frame(width: 250)
    }

    private func chooseOutputAndGenerate() {
        guard let targetVolumeID, let keepVolumeID else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "disk-indexer-cleanup-plan.json"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        appState.generateCleanupPlan(
            targetVolumeID: targetVolumeID,
            keepVolumeID: keepVolumeID,
            minimumCopies: minimumCopies,
            minimumDevices: minimumDevices,
            verifyMetadata: verifyMetadata,
            verifyFullHash: verifyFullHash,
            outputURL: url
        )
    }
}

struct LogsView: View {
    @EnvironmentObject private var appState: AppState
    @State private var searchText = ""
    @State private var level: AppLogEntry.Level?

    private var entries: [AppLogEntry] {
        Array(appState.logs.filter { entry in
            (level == nil || entry.level == level) &&
                (searchText.isEmpty || entry.message.localizedCaseInsensitiveContains(searchText))
        }.reversed())
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("日志").font(.largeTitle.bold())
                Spacer()
                Picker("级别", selection: $level) {
                    Text("全部").tag(Optional<AppLogEntry.Level>.none)
                    ForEach(AppLogEntry.Level.allCases) { level in Text(level.label).tag(Optional(level)) }
                }
                Button("导出诊断…") { exportLogs() }
            }
            TextField("搜索诊断消息", text: $searchText)
            Text("日志只保存操作诊断和路径，不记录文件内容；内存中最多保留 500 条。")
                .font(.caption)
                .foregroundStyle(.secondary)
            List(entries) { entry in
                VStack(alignment: .leading, spacing: 3) {
                    Text("\(entry.level.label) · \(entry.operation)").font(.headline)
                    Text(entry.message).textSelection(.enabled)
                    Text(entry.timestamp.formatted()).font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .padding(24)
    }

    private func exportLogs() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "disk-indexer-diagnostics.tsv"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            try appState.exportLogs(to: url)
        } catch {
            appState.errorMessage = error.localizedDescription
        }
    }
}

private func copyStatusIcon(_ status: String) -> String {
    switch status {
    case "present": "checkmark.circle"
    case "offline_unverified": "externaldrive.badge.exclamationmark"
    case "missing": "questionmark.circle"
    default: "exclamationmark.triangle"
    }
}
