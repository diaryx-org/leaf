//  PictureDragTests.swift
//
//  A picture dragged out of the macOS editor: its file where it has one, a
//  promise of one where its bytes live in the markup, the block's text
//  alongside either way, and nothing for a box that has drawn no picture.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import UniformTypeIdentifiers
import LeafFFI
@testable import LeafUI

final class PictureDragTests: XCTestCase {
    private static let dotBase64 =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="

    private var dir: URL!

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("leaf-drag-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try XCTUnwrap(Data(base64Encoded: Self.dotBase64)).write(to: dir.appendingPathComponent("dot.png"))
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: dir)
    }

    private func editor(_ source: String) throws -> LeafTextView {
        let view = LeafTextView(doc: try LeafDoc(source: source, format: "markdown"), theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.documentDirectory = dir
        view.layout()
        return view
    }

    private func picture(in view: LeafTextView) throws -> MediaView {
        try XCTUnwrap(view.layoutEngine.rows.lazy.compactMap(\.media).first?.media)
    }

    func testAPictureInAFileIsDraggedAsThatFile() throws {
        let view = try editor("![dot](dot.png)\n")
        let writer = try XCTUnwrap(view.pictureDragWriter(for: try picture(in: view), text: "![dot](dot.png)"))
        let item = try XCTUnwrap(writer as? NSPasteboardItem)
        let url = try XCTUnwrap(item.string(forType: .fileURL).flatMap(URL.init(string:)))
        XCTAssertEqual(url.standardizedFileURL.path, dir.appendingPathComponent("dot.png").standardizedFileURL.path)
        XCTAssertEqual(item.string(forType: .string), "![dot](dot.png)", "the markup, for a receiver of text")
    }

    func testAPictureInTheMarkupIsAPromisedFile() throws {
        let src = "data:image/png;base64," + Self.dotBase64
        let view = try editor("![tiny dot](\(src))\n")
        let writer = try XCTUnwrap(view.pictureDragWriter(for: try picture(in: view), text: "block"))
        let provider = try XCTUnwrap(writer as? PictureFilePromiseProvider)
        XCTAssertEqual(provider.fileType, UTType.png.identifier)
        XCTAssertTrue(provider.writableTypes(for: .general).contains(.string))
        XCTAssertEqual(provider.pasteboardPropertyList(forType: .string) as? String, "block")
        let promise = try XCTUnwrap(provider.promise)
        XCTAssertEqual(promise.filePromiseProvider(provider, fileNameForType: provider.fileType), "tiny dot.png")
        let target = dir.appendingPathComponent("out.png")
        let written = expectation(description: "written")
        promise.filePromiseProvider(provider, writePromiseTo: target) { error in
            XCTAssertNil(error)
            written.fulfill()
        }
        wait(for: [written], timeout: 1)
        XCTAssertEqual(try Data(contentsOf: target), Data(base64Encoded: Self.dotBase64))
    }

    func testABoxWithNoPictureGivesNothing() throws {
        let view = try editor("![gone](missing.png)\n")
        XCTAssertNil(view.pictureDragWriter(for: try picture(in: view), text: "x"))
    }
}
#endif
