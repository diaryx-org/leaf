import XCTest
@testable import LeafUI

/// What a followed link hands the system once the host has declined it.
final class LinkDestinationTests: XCTestCase {
    private let folder = URL(fileURLWithPath: "/Users/reader/Notes", isDirectory: true)

    private func resolve(_ destination: String, in directory: URL? = nil) -> URL? {
        LinkDestination.url(for: destination, relativeTo: directory ?? folder)
    }

    /// The bug: a bare `notes.txt` went to Launch Services as a URL with no
    /// scheme and no base, which it refuses with error -50.
    func testARelativePathIsAFileBesideTheDocument() {
        XCTAssertEqual(resolve("notes.txt"), URL(fileURLWithPath: "/Users/reader/Notes/notes.txt"))
        XCTAssertEqual(resolve("./notes.txt"), URL(fileURLWithPath: "/Users/reader/Notes/notes.txt"))
        XCTAssertEqual(resolve("../2026/july.md"), URL(fileURLWithPath: "/Users/reader/2026/july.md"))
    }

    func testAURLGoesAsItIs() {
        XCTAssertEqual(resolve("https://example.com/a?b#c"), URL(string: "https://example.com/a?b#c"))
        XCTAssertEqual(resolve("mailto:reader@example.com"), URL(string: "mailto:reader@example.com"))
        XCTAssertEqual(resolve("file:///tmp/a.txt"), URL(string: "file:///tmp/a.txt"))
    }

    func testALeadingSlashIsTheRootOfTheFileSystem() {
        XCTAssertEqual(resolve("/etc/hosts"), URL(fileURLWithPath: "/etc/hosts"))
    }

    func testAPlaceInTheFileIsDroppedAndEscapesAreDecoded() {
        XCTAssertEqual(resolve("my%20draft.md#v2"), URL(fileURLWithPath: "/Users/reader/Notes/my draft.md"))
        XCTAssertEqual(resolve("page.html?x=1"), URL(fileURLWithPath: "/Users/reader/Notes/page.html"))
        XCTAssertEqual(resolve("my draft.md"), URL(fileURLWithPath: "/Users/reader/Notes/my draft.md"))
    }

    func testNothingToResolveAgainstIsNothing() {
        XCTAssertNil(LinkDestination.url(for: "notes.txt", relativeTo: nil))
        XCTAssertNil(resolve("   "))
        XCTAssertNil(resolve("#v2"))
    }

    func testAMissingFileIsNotOpenable() {
        XCTAssertFalse(LinkDestination.isOpenable(URL(fileURLWithPath: "/nonexistent/leaf/notes.txt")))
        XCTAssertTrue(LinkDestination.isOpenable(URL(fileURLWithPath: "/etc/hosts")))
        XCTAssertTrue(LinkDestination.isOpenable(URL(string: "https://example.com")!))
    }
}
