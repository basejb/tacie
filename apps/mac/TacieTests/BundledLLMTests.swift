import Darwin
import XCTest
@testable import Tacie

/// Tacie's own notes server (issue #54): how it is started. The process itself runs in the app, not in tests.
final class BundledLLMTests: XCTestCase {
    func testSlotsFollowMemory() {
        XCTAssertEqual(BundledLLM.slots(memory: 16 << 30), 2)
        XCTAssertEqual(BundledLLM.slots(memory: 24 << 30), 4)
        XCTAssertEqual(BundledLLM.slots(memory: 64 << 30), 4)
    }

    func testArguments() {
        let model = URL(fileURLWithPath: "/tmp/m/gemma.gguf")
        let a = BundledLLM.arguments(model: model, port: 11_535, memory: 16 << 30)
        func value(_ flag: String) -> String? { a.firstIndex(of: flag).map { a[$0 + 1] } }
        XCTAssertEqual(value("-m"), "/tmp/m/gemma.gguf")
        XCTAssertEqual(value("--host"), "127.0.0.1")   // never reachable from other machines
        XCTAssertEqual(value("--port"), "11535")
        XCTAssertEqual(value("--alias"), BundledLLM.model)
        XCTAssertEqual(value("-np"), "2")
        XCTAssertEqual(value("-c"), String(2 * BundledLLM.slotContext))
        XCTAssertEqual(value("--reasoning"), "off")
        XCTAssertEqual(value("--sleep-idle-seconds"), String(BundledLLM.idleSeconds))
        XCTAssertTrue(a.contains("--no-mmproj"))
    }

    func testFreePortPrefersTheGivenOneAndFallsBack() throws {
        let p = BundledLLM.freePort(preferred: 0)
        XCTAssertGreaterThan(p, 0)
        // hold a port, then ask for it: another one comes back
        let held = BundledLLM.freePort(preferred: 0)
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        defer { close(fd) }
        var addr = sockaddr_in()
        addr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = in_port_t(UInt16(held).bigEndian)
        addr.sin_addr.s_addr = inet_addr("127.0.0.1")
        let ok = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) }
        }
        try XCTSkipIf(ok != 0, "could not hold a port")
        XCTAssertNotEqual(BundledLLM.freePort(preferred: held), held)
    }

    func testModelFileOnlyWhenComplete() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: dir) }
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let env = ["TACIE_NOTES_MODEL_DIR": dir.path]
        XCTAssertNil(BundledLLM.modelFile(env: env))
        // a partial file (still downloading) does not count
        try Data(count: 10).write(to: dir.appendingPathComponent(ModelManifest.notesFile.name))
        XCTAssertNil(BundledLLM.modelFile(env: env))
    }
}
