import XCTest
@testable import DiskIndexer

final class DiskIndexerAppTests: XCTestCase {
    func testRunnerUsesExplicitDatabaseAndArgumentArray() {
        let runner = RustCommandRunner(toolURL: URL(fileURLWithPath: "/tmp/disk-indexer"))
        XCTAssertEqual(
            runner.commandArguments(databasePath: "/tmp/index.db", command: ["volume", "list", "--json"]),
            ["--db", "/tmp/index.db", "volume", "list", "--json"]
        )
    }

    func testJSONLineDecoderHandlesFragmentedUTF8Lines() throws {
        let decoder = JSONLineDecoder()
        let line = """
        {"protocol_version":1,"type":"progress","task_id":"task-1","timestamp":"2026-07-29T00:00:00Z","operation":"scan","files_seen":7,"current_path":"/tmp/a"}
        """ + "\n"
        let data = try XCTUnwrap(line.data(using: .utf8))
        let split = data.index(data.startIndex, offsetBy: 31)
        XCTAssertTrue(decoder.append(Data(data[..<split])).isEmpty)
        let results = decoder.append(Data(data[split...]))
        XCTAssertEqual(results.count, 1)
        guard case let .success(event) = try XCTUnwrap(results.first) else {
            return XCTFail("应解码为 JSONL 进度事件")
        }
        XCTAssertEqual(event.type, "progress")
        XCTAssertEqual(event.filesSeen, 7)
        XCTAssertEqual(event.currentPath, "/tmp/a")
    }

    func testCancelledTaskStateIsNotRunning() {
        XCTAssertFalse(TaskProcessStatus.cancelled.isRunning)
        XCTAssertEqual(TaskProcessStatus.cancelled.label, "已取消")
    }
}
