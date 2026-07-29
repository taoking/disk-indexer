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
}
