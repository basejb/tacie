import XCTest
@testable import Tacie

final class BrandImagesTests: XCTestCase {
    func testTemplateImageHasHoles() throws {
        let img = BrandImages.menuBarTemplate(pointHeight: 14)
        XCTAssertTrue(img.isTemplate)
        XCTAssertEqual(img.size.width, 14 * 1.5, accuracy: 0.01)
        let tiff = try XCTUnwrap(img.tiffRepresentation)
        let rep = try XCTUnwrap(NSBitmapImageRep(data: tiff))
        let w = rep.pixelsWide, h = rep.pixelsHigh
        let center = try XCTUnwrap(rep.colorAt(x: w / 2, y: h / 2)).alphaComponent     // head: filled
        let leftEye = try XCTUnwrap(rep.colorAt(x: w * 3 / 10, y: h * 58 / 100)).alphaComponent    // hole: clear (bitmap y runs from the top, like the eye line)
        XCTAssertGreaterThan(center, 0.9)
        XCTAssertLessThan(leftEye, 0.1)
    }

    @MainActor func testExportedAvatarsAreSquare() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("tacie-brand-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        try BrandImages.exportBrand(to: dir)
        for name in ["slack-512.png", "l1-dark-512.png", "AppIcon-1024.png"] {
            let img = try XCTUnwrap(NSImage(contentsOf: dir.appendingPathComponent(name)), name)
            let rep = try XCTUnwrap(img.representations.first, name)
            XCTAssertEqual(rep.pixelsWide, rep.pixelsHigh, name)   // Slack and the App Store want squares
            XCTAssertEqual(rep.pixelsWide, name.hasPrefix("AppIcon") ? 1024 : 512, name)
        }
    }
}
