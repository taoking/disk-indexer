import AppKit
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private weak var taskController: TaskProcessController?
    private var waitingForTaskTermination = false

    func bind(taskController: TaskProcessController) {
        self.taskController = taskController
        taskController.onStateFinished = { [weak self] _ in
            guard let self, self.waitingForTaskTermination else { return }
            self.waitingForTaskTermination = false
            NSApp.reply(toApplicationShouldTerminate: true)
        }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let taskController, taskController.status.isRunning else { return .terminateNow }
        let alert = NSAlert()
        alert.messageText = "本地索引任务仍在运行"
        alert.informativeText = "为避免留下孤儿 Rust 子进程，请选择继续等待，或先安全取消任务后退出。"
        alert.addButton(withTitle: "取消任务并退出")
        alert.addButton(withTitle: "继续等待")
        alert.addButton(withTitle: "不关闭")
        switch alert.runModal() {
        case .alertFirstButtonReturn:
            waitingForTaskTermination = true
            taskController.cancel()
            return .terminateLater
        case .alertSecondButtonReturn, .alertThirdButtonReturn:
            return .terminateCancel
        default:
            return .terminateCancel
        }
    }
}

@main
struct DiskIndexerApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var appState = AppState()

    var body: some Scene {
        WindowGroup("Disk Indexer") {
            RootView()
                .environmentObject(appState)
                .frame(minWidth: 920, minHeight: 620)
                .onAppear { appDelegate.bind(taskController: appState.taskController) }
        }
        .defaultSize(width: 1120, height: 760)
    }
}
