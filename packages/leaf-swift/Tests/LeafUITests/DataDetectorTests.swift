//  DataDetectorTests.swift
//
//  Dates, addresses, phone numbers and bare URLs in the macOS view's prose:
//  found under a point by the system's data detectors, as whole phrases in
//  source bytes, and handed to the app the system has for each.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class DataDetectorTests: XCTestCase {
    private func editor(_ source: String) throws -> LeafTextView {
        let view = LeafTextView(doc: try LeafDoc(source: source, format: "markdown"), theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 800, height: 400)
        view.layout()
        return view
    }

    /// The match at the first byte of `needle`, which must be in row 0.
    private func detected(_ needle: String, in view: LeafTextView, row: Int = 0) -> LeafTextView.DetectedData? {
        let source = view.sourceText()
        guard let r = source.range(of: needle) else { return nil }
        let off = source.utf8.distance(from: source.startIndex, to: r.lowerBound) + 1
        return view.detectedData(atByte: off, row: row)
    }

    func testAPhoneNumberIsFoundWholeAndCalled() throws {
        let view = try editor("Call me on (555) 123-4567 tomorrow.\n")
        let found = try XCTUnwrap(detected("555", in: view))
        XCTAssertEqual(found.result.resultType, .phoneNumber)
        XCTAssertTrue(found.text.contains("123-4567"))
        let action = LeafTextView.dataDetectorAction(for: found.result, text: found.text)
        XCTAssertEqual(action, .open(URL(string: "tel:5551234567")!))
    }

    func testABareURLIsOpened() throws {
        let view = try editor("Read https://example.com/page now\n")
        let found = try XCTUnwrap(detected("example", in: view))
        XCTAssertEqual(found.result.resultType, .link)
        guard case .open(let url) = LeafTextView.dataDetectorAction(for: found.result, text: found.text) else {
            return XCTFail("a URL is opened")
        }
        XCTAssertEqual(url.host, "example.com")
    }

    func testADateIsAnEventForCalendar() throws {
        let view = try editor("Lunch on October 5, 2026 at 1pm with Sam\n")
        let found = try XCTUnwrap(detected("October", in: view))
        XCTAssertEqual(found.result.resultType, .date)
        // The whole phrase, in source bytes: Look Up and the menu act on it.
        let text = view.doc.textInRange(from: UInt32(found.from), to: UInt32(found.to))
        XCTAssertTrue(text.contains("October 5, 2026 at 1pm"), text)
        XCTAssertEqual(text, found.text)
        let date = try XCTUnwrap(found.result.date)
        let ics = LeafTextView.calendarEvent(starting: date, lasting: 0, title: "Lunch, with Sam")
        XCTAssertTrue(ics.contains("BEGIN:VEVENT"))
        XCTAssertTrue(ics.contains("SUMMARY:Lunch\\, with Sam"), "commas are escaped in iCalendar text")
        XCTAssertTrue(ics.contains("DTEND:"))
        // Resolving the action writes nothing; only taking it does.
        guard case .addEvent(let event) = LeafTextView.dataDetectorAction(for: found.result, text: found.text) else {
            return XCTFail("a date is an event to add")
        }
        XCTAssertTrue(event.contains("BEGIN:VEVENT"))
    }

    func testAnEventFileReplacesTheLastOne() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("LeafTests-Events-\(UUID().uuidString)", isDirectory: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        let first = try XCTUnwrap(LeafTextView.writeCalendarEvent("one", in: directory))
        let second = try XCTUnwrap(LeafTextView.writeCalendarEvent("two", in: directory))
        XCTAssertEqual(second.pathExtension, "ics")
        XCTAssertFalse(FileManager.default.fileExists(atPath: first.path), "the last event goes")
        let left = try FileManager.default.contentsOfDirectory(atPath: directory.path)
        XCTAssertEqual(left, [second.lastPathComponent])
        XCTAssertEqual(try String(contentsOf: second, encoding: .utf8), "two")
    }

    func testAnAddressGoesToMaps() throws {
        let view = try editor("Office: 1 Apple Park Way, Cupertino, CA 95014 is where.\n")
        let found = try XCTUnwrap(detected("Apple Park", in: view))
        XCTAssertEqual(found.result.resultType, .address)
        guard case .open(let url) = LeafTextView.dataDetectorAction(for: found.result, text: found.text) else {
            return XCTFail("an address is opened in Maps")
        }
        XCTAssertEqual(url.host, "maps.apple.com")
        XCTAssertTrue(url.query?.contains("Cupertino") == true)
    }

    func testPlainWordsFindNothing() throws {
        let view = try editor("Nothing to see here\n")
        XCTAssertNil(detected("see", in: view))
    }

    func testHiddenMarkupDoesNotShiftTheMatch() throws {
        let view = try editor("**Note:** call (555) 123-4567\n")
        let found = try XCTUnwrap(detected("555", in: view))
        XCTAssertEqual(view.doc.textInRange(from: UInt32(found.from), to: UInt32(found.to)), found.text)
    }
}
#endif
