import Foundation

struct CommandOutput: Sendable {
    let stdout: Data
    let stderr: Data
}

struct RustCommandError: LocalizedError, Sendable {
    let message: String

    var errorDescription: String? { message }
}

private final class PipeCapture: @unchecked Sendable {
    private let lock = NSLock()
    private var value = Data()

    func set(_ data: Data) {
        lock.lock()
        value = data
        lock.unlock()
    }

    func get() -> Data {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

/// 唯一的 Rust 调用边界。只使用 App Bundle 内资源和 Process.arguments，绝不经过 shell 或网络。
struct RustCommandRunner {
    let toolURL: URL?

    init(toolURL: URL? = Bundle.main.url(forResource: "disk-indexer", withExtension: nil)) {
        self.toolURL = toolURL
    }

    func commandArguments(databasePath: String, command: [String]) -> [String] {
        ["--db", databasePath] + command
    }

    func runJSON<T: Decodable>(
        databasePath: String,
        command: [String],
        as type: T.Type = T.self
    ) async throws -> T {
        let output = try await run(databasePath: databasePath, command: command)
        do {
            return try JSONDecoder().decode(T.self, from: output.stdout)
        } catch {
            throw RustCommandError(
                message: "Rust CLI 返回的 JSON 无法解析：\(error.localizedDescription)\n\(String(data: output.stdout, encoding: .utf8) ?? "")"
            )
        }
    }

    func run(databasePath: String, command: [String]) async throws -> CommandOutput {
        guard let toolURL else {
            throw RustCommandError(
                message: "App Bundle 中缺少 disk-indexer 工具。请使用 scripts/build-macos-app.sh 重新构建应用。"
            )
        }
        let arguments = commandArguments(databasePath: databasePath, command: command)
        return try await Task.detached(priority: .userInitiated) {
            try RustCommandRunner.runBlocking(toolURL: toolURL, arguments: arguments)
        }.value
    }

    private static func runBlocking(toolURL: URL, arguments: [String]) throws -> CommandOutput {
        let process = Process()
        let stdout = Pipe()
        let stderr = Pipe()
        process.executableURL = toolURL
        process.arguments = arguments
        process.standardOutput = stdout
        process.standardError = stderr
        do {
            try process.run()
        } catch {
            throw RustCommandError(message: "无法启动内置 Rust CLI：\(error.localizedDescription)")
        }
        // 必须在等待子进程前持续读取两个管道；否则大 JSON 报告可能写满管道而互相等待。
        let group = DispatchGroup()
        let stdoutCapture = PipeCapture()
        let stderrCapture = PipeCapture()
        group.enter()
        DispatchQueue.global(qos: .userInitiated).async {
            stdoutCapture.set(stdout.fileHandleForReading.readDataToEndOfFile())
            group.leave()
        }
        group.enter()
        DispatchQueue.global(qos: .userInitiated).async {
            stderrCapture.set(stderr.fileHandleForReading.readDataToEndOfFile())
            group.leave()
        }
        process.waitUntilExit()
        group.wait()
        let stdoutData = stdoutCapture.get()
        let stderrData = stderrCapture.get()
        guard process.terminationStatus == 0 else {
            let diagnostics = String(data: stderrData, encoding: .utf8) ?? "无诊断输出"
            throw RustCommandError(message: "Rust CLI 退出码 \(process.terminationStatus)：\(diagnostics)")
        }
        return CommandOutput(stdout: stdoutData, stderr: stderrData)
    }
}
