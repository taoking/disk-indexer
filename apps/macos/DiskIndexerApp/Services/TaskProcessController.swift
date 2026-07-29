import Combine
import Foundation

enum TaskProcessStatus: Equatable {
    case idle
    case running
    case cancelling
    case cancelled
    case completed
    case failed(String)

    var isRunning: Bool {
        self == .running || self == .cancelling
    }

    var label: String {
        switch self {
        case .idle: "待命"
        case .running: "运行中"
        case .cancelling: "正在取消"
        case .cancelled: "已取消"
        case .completed: "已完成"
        case .failed: "失败"
        }
    }
}

/// 将任意字节片段复原为完整 JSON Lines；stdout 不允许混入普通文本。
final class JSONLineDecoder {
    static let supportedProtocolVersion = 1
    private var pending = Data()
    private let decoder = JSONDecoder()

    func append(_ data: Data) -> [Result<JSONLTaskEvent, RustCommandError>] {
        pending.append(data)
        return extractCompleteLines()
    }

    func finish() -> [Result<JSONLTaskEvent, RustCommandError>] {
        defer { pending.removeAll(keepingCapacity: false) }
        guard !pending.isEmpty else { return [] }
        return decodeLine(pending)
    }

    private func extractCompleteLines() -> [Result<JSONLTaskEvent, RustCommandError>] {
        var results: [Result<JSONLTaskEvent, RustCommandError>] = []
        while let newline = pending.firstIndex(of: 0x0A) {
            let line = pending[..<newline]
            pending.removeSubrange(...newline)
            guard !line.isEmpty else { continue }
            results.append(contentsOf: decodeLine(Data(line)))
        }
        return results
    }

    private func decodeLine(_ line: Data) -> [Result<JSONLTaskEvent, RustCommandError>] {
        do {
            let event = try decoder.decode(JSONLTaskEvent.self, from: line)
            guard event.protocolVersion == Self.supportedProtocolVersion else {
                return [.failure(RustCommandError(
                    message: "不支持的 JSONL 协议版本 \(event.protocolVersion)；App 仅支持 \(Self.supportedProtocolVersion)。"
                ))]
            }
            return [.success(event)]
        } catch {
            let fragment = String(decoding: line.prefix(300), as: UTF8.self)
            return [.failure(RustCommandError(message: "JSONL 事件无法解析：\(error.localizedDescription)\n\(fragment)"))]
        }
    }
}

/// 长任务的唯一进程控制器。它只运行 Bundle 内 CLI，实时分离 stdout JSONL 与 stderr 诊断。
@MainActor
final class TaskProcessController: ObservableObject {
    static let maximumRetainedEvents = 500
    @Published private(set) var status: TaskProcessStatus = .idle
    @Published private(set) var currentEvent: JSONLTaskEvent?
    @Published private(set) var events: [JSONLTaskEvent] = []
    @Published private(set) var diagnostics: [String] = []

    var onEvent: ((JSONLTaskEvent) -> Void)?
    var onDiagnostic: ((String) -> Void)?
    var onFinish: ((TaskProcessStatus) -> Void)?
    var onStateFinished: ((TaskProcessStatus) -> Void)?

    private var process: Process?
    private var lineDecoder = JSONLineDecoder()
    private var terminationWasRequested = false

    func start(
        runner: RustCommandRunner,
        databasePath: String,
        command: [String]
    ) throws {
        guard !status.isRunning else {
            throw RustCommandError(message: "已有本地任务正在运行；请先等待完成或取消。")
        }
        guard let toolURL = runner.toolURL else {
            throw RustCommandError(message: "App Bundle 中缺少 disk-indexer 工具，请重新构建应用。")
        }
        let process = Process()
        let stdout = Pipe()
        let stderr = Pipe()
        lineDecoder = JSONLineDecoder()
        events = []
        diagnostics = []
        currentEvent = nil
        terminationWasRequested = false
        process.executableURL = toolURL
        process.arguments = runner.commandArguments(databasePath: databasePath, command: command)
        process.standardOutput = stdout
        process.standardError = stderr
        stdout.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard !data.isEmpty else { return }
            Task { @MainActor [weak self] in
                self?.consumeStdout(data)
            }
        }
        stderr.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard !data.isEmpty else { return }
            let message = String(decoding: data, as: UTF8.self)
            Task { @MainActor [weak self] in
                self?.recordDiagnostic(message)
            }
        }
        process.terminationHandler = { [weak self] process in
            Task { @MainActor [weak self] in
                self?.finish(process: process, stdout: stdout, stderr: stderr)
            }
        }
        self.process = process
        status = .running
        do {
            try process.run()
        } catch {
            self.process = nil
            status = .failed(error.localizedDescription)
            throw RustCommandError(message: "无法启动内置 Rust CLI：\(error.localizedDescription)")
        }
    }

    func cancel() {
        guard let process, process.isRunning, status == .running else { return }
        terminationWasRequested = true
        status = .cancelling
        process.interrupt()
        DispatchQueue.main.asyncAfter(deadline: .now() + 5) { [weak self] in
            guard let self, self.status == .cancelling, process.isRunning else { return }
            // 正常取消先发送 SIGINT；仅在进程没有响应时才温和 terminate，绝不使用 kill -9。
            process.terminate()
        }
    }

    private func consumeStdout(_ data: Data) {
        for result in lineDecoder.append(data) {
            switch result {
            case let .success(event):
                recordEvent(event)
            case let .failure(error):
                recordDiagnostic(error.message)
            }
        }
    }

    private func recordDiagnostic(_ message: String) {
        let normalized = message.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalized.isEmpty else { return }
        diagnostics.append(normalized)
        onDiagnostic?(normalized)
    }

    private func finish(process: Process, stdout: Pipe, stderr: Pipe) {
        stdout.fileHandleForReading.readabilityHandler = nil
        stderr.fileHandleForReading.readabilityHandler = nil
        consumeStdout(stdout.fileHandleForReading.readDataToEndOfFile())
        recordDiagnostic(String(decoding: stderr.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self))
        for result in lineDecoder.finish() {
            switch result {
            case let .success(event):
                recordEvent(event)
            case let .failure(error):
                recordDiagnostic(error.message)
            }
        }
        self.process = nil
        if terminationWasRequested {
            status = .cancelled
        } else if process.terminationStatus == 0 {
            status = .completed
        } else {
            let reason = diagnostics.last ?? "Rust CLI 退出码 \(process.terminationStatus)"
            status = .failed(reason)
        }
        onFinish?(status)
        onStateFinished?(status)
    }

    private func recordEvent(_ event: JSONLTaskEvent) {
        currentEvent = event
        events = Self.retainingRecentEvents(events, appending: event)
        onEvent?(event)
    }

    static func retainingRecentEvents(_ existing: [JSONLTaskEvent], appending event: JSONLTaskEvent) -> [JSONLTaskEvent] {
        let updated = existing + [event]
        guard updated.count > maximumRetainedEvents else { return updated }
        return Array(updated.suffix(maximumRetainedEvents))
    }
}
