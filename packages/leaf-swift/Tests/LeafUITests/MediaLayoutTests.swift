//  MediaLayoutTests.swift
//
//  The geometry of a block media box: how it fits a picture, how it collapses
//  core's reserved rows onto one box, and what it resolves a `src` to.
//
//  No `MediaStore` is handed to the layout here, so every box lays out at its
//  no-picture size — which is the point: the collapsing and the row bookkeeping
//  are what these test, and neither should need a file on disk to exercise.

import CoreGraphics
import XCTest

@testable import LeafUI
import LeafFFI

final class MediaLayoutTests: XCTestCase {
    private let theme = EditorTheme.default

    // MARK: fitting

    func testFitScalesDownToTheBoxKeepingAspect() {
        let out = MediaLayout.fit(CGSize(width: 800, height: 400), maxWidth: 400, maxHeight: 1000)
        XCTAssertEqual(out.width, 400)
        XCTAssertEqual(out.height, 200, "aspect held: half the width, half the height")
    }

    func testFitIsCappedByHeightAsWellAsWidth() {
        // A tall, narrow picture is bounded by the height cap, not the column.
        let out = MediaLayout.fit(CGSize(width: 100, height: 1000), maxWidth: 400, maxHeight: 200)
        XCTAssertEqual(out.height, 200)
        XCTAssertEqual(out.width, 20)
    }

    func testFitNeverEnlargesASmallPicture() {
        // A 16pt icon blown up to span the text column reads as a bug, not a
        // feature — the scale is clamped at 1.
        let out = MediaLayout.fit(CGSize(width: 16, height: 16), maxWidth: 600, maxHeight: 600)
        XCTAssertEqual(out, CGSize(width: 16, height: 16))
    }

    func testFitSurvivesADegenerateSize() {
        // A zero dimension would divide by zero; it falls back to the full box.
        let out = MediaLayout.fit(CGSize(width: 0, height: 0), maxWidth: 300, maxHeight: 100)
        XCTAssertEqual(out, CGSize(width: 300, height: 100))
    }

    // MARK: kinds

    func testAudioGetsAFixedControlHeightNotAnAspectRatio() {
        let box = MediaLayout(mkMedia("take.mp3", kind: .audio), still: nil,
                              contentWidth: 900, theme: theme)
        XCTAssertEqual(box.size.height, MediaMetrics.audioHeight)
        XCTAssertEqual(box.size.width, MediaMetrics.audioWidth,
                       "capped rather than spanning a wide column")
        XCTAssertTrue(box.showsPlayBadge)
        XCTAssertFalse(box.isBroken, "audio has no picture to be missing")
    }

    func testAPosterlessVideoIsReservedAtVideoShapeNotAsAChip() {
        // It has no poster to measure, but it is still a picture: reserving a
        // chip would make the player letterbox a whole movie into a strip the
        // height of a line of text the moment it starts.
        let box = MediaLayout(mkMedia("clip.mp4", kind: .video), still: nil,
                              contentWidth: 600, theme: theme)
        XCTAssertTrue(box.showsPlayBadge)
        XCTAssertFalse(box.isBroken, "a video with no poster isn't broken, just pictureless")
        XCTAssertEqual(box.size.width / box.size.height, 16.0 / 9.0, accuracy: 0.02)
        XCTAssertGreaterThan(box.size.height, MediaMetrics.audioHeight * 2,
                             "not the chip height")
    }

    func testABrokenImageIsStillAChip() {
        // The chip is for something with nothing to show *and* nothing expected.
        let box = MediaLayout(mkMedia("gone.png"), still: nil, contentWidth: 600, theme: theme)
        XCTAssertEqual(box.size.width, MediaMetrics.chipWidth)
        XCTAssertEqual(box.chipLabel, "gone.png")
    }

    func testAnImageThatDidNotLoadReadsAsBroken() {
        // The one case that earns the dashed frame: an image is *only* a picture,
        // so no picture means the path is wrong.
        let box = MediaLayout(mkMedia("gone.png"), still: nil, contentWidth: 600, theme: theme)
        XCTAssertTrue(box.isBroken)
        XCTAssertFalse(box.showsPlayBadge, "nothing to play")
    }

    func testChipLabelPrefersAltThenTheFileName() {
        XCTAssertEqual(
            MediaLayout(mkMedia("a/b/clip.mp4", kind: .video, alt: "the talk"), still: nil,
                        contentWidth: 400, theme: theme).chipLabel,
            "the talk")
        XCTAssertEqual(
            MediaLayout(mkMedia("a/b/clip.mp4", kind: .video), still: nil,
                        contentWidth: 400, theme: theme).chipLabel,
            "clip.mp4", "the path's last component, not the whole path")
    }

    // MARK: collapsing into the frame

    func testMediaRowsCollapseOntoOneBoxAndStayOneToOneWithCoreRows() {
        // Core reserved three rows; the box replaces them. `rows` must stay 1:1
        // with the frame — every caret and click path indexes it by core's row
        // number — so all three survive, sharing one top, with only the first
        // carrying height. The same contract a table's picture rows hold.
        let frame = docView(
            [row([mkRun("before")]),
             row([mkRun("🖼 a cat")]), row([]), row([]),
             row([mkRun("after")])],
            media: [mkMedia("cat.png", startRow: 1, endRow: 4)]
        )
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400)

        XCTAssertEqual(layout.rows.count, 5, "rows stay 1:1 with core's rows")
        let boxes = layout.rows.filter { $0.media != nil }
        XCTAssertEqual(boxes.count, 3, "all three reserved rows carry the box")
        XCTAssertEqual(boxes.filter(\.mediaFirst).count, 1, "exactly one paints it")
        XCTAssertEqual(Set(boxes.map(\.mediaTop)).count, 1, "they share one top")
        XCTAssertEqual(boxes.filter { !$0.mediaFirst }.map(\.height), [0, 0],
                       "only the first row has height")
        XCTAssertGreaterThan(boxes[0].height, 0)
    }

    func testTheRowAfterMediaStartsBelowTheWholeBox() {
        let frame = docView(
            [row([mkRun("🖼 a cat")]), row([]), row([mkRun("after")])],
            media: [mkMedia("cat.png", startRow: 0, endRow: 2)]
        )
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400)
        let box = layout.rows[0]
        XCTAssertEqual(layout.rows[2].top, box.top + box.height, accuracy: 0.5,
                       "prose resumes under the box, not under the reserved rows")
    }

    func testAFrameWithNoMediaIsUntouched() {
        let frame = docView([row([mkRun("just prose")]), row([mkRun("more")])])
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400)
        XCTAssertTrue(layout.rows.allSatisfy { $0.media == nil })
    }

    // MARK: hit-testing

    func testOnlyPlayableMediaAnswersAHit() {
        // An image has nothing to activate, so a click on one must fall through
        // to ordinary caret placement rather than being swallowed.
        let frame = docView(
            [row([mkRun("🖼 a cat")]), row([mkRun("🎬 clip")])],
            media: [mkMedia("cat.png", startRow: 0, endRow: 1),
                    mkMedia("clip.mp4", kind: .video, startRow: 1, endRow: 2)]
        )
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400)

        let image = layout.rows[0]
        let inImage = CGPoint(x: theme.padding.left + 4, y: image.mediaTop + MediaMetrics.gap + 4)
        XCTAssertNil(layout.playableMedia(at: inImage, theme: theme), "an image isn't playable")

        let video = layout.rows[1]
        let inVideo = CGPoint(x: theme.padding.left + 4, y: video.mediaTop + MediaMetrics.gap + 4)
        XCTAssertEqual(layout.playableMedia(at: inVideo, theme: theme)?.src, "clip.mp4")
    }

    func testAPointOutsideEveryBoxHitsNothing() {
        let frame = docView([row([mkRun("🎬 clip")])],
                            media: [mkMedia("clip.mp4", kind: .video, startRow: 0, endRow: 1)])
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400)
        XCTAssertNil(layout.playableMedia(at: CGPoint(x: 5000, y: 5000), theme: theme))
    }

    // MARK: the rects a player is positioned onto

    func testMediaRectsAreKeyedBySrcNotByRow() {
        // Keying by src is what lets a playing video survive an edit above it:
        // rows renumber on every keystroke, sources don't.
        let frame = docView(
            [row([mkRun("🖼 a cat")]), row([mkRun("🎬 clip")])],
            media: [mkMedia("cat.png", startRow: 0, endRow: 1),
                    mkMedia("clip.mp4", kind: .video, startRow: 1, endRow: 2)]
        )
        let rects = EditorLayout(frame, theme: theme, wrapWidth: 400).mediaRects(theme: theme)
        XCTAssertEqual(Set(rects.keys), ["cat.png", "clip.mp4"])
    }

    func testAnEditAboveAVideoMovesItsRectWithoutChangingItsKey() {
        // The scenario the keying exists for. Same document, one extra line of
        // prose above: the video's rect moves down, its key is untouched — so a
        // host repositions the installed player instead of tearing it down.
        let before = docView(
            [row([mkRun("intro")]), row([mkRun("🎬 clip")])],
            media: [mkMedia("clip.mp4", kind: .video, startRow: 1, endRow: 2)]
        )
        let after = docView(
            [row([mkRun("intro")]), row([mkRun("a new line")]), row([mkRun("🎬 clip")])],
            media: [mkMedia("clip.mp4", kind: .video, startRow: 2, endRow: 3)]
        )
        let r1 = EditorLayout(before, theme: theme, wrapWidth: 400).mediaRects(theme: theme)
        let r2 = EditorLayout(after, theme: theme, wrapWidth: 400).mediaRects(theme: theme)
        XCTAssertNotNil(r1["clip.mp4"])
        XCTAssertNotNil(r2["clip.mp4"])
        XCTAssertGreaterThan(r2["clip.mp4"]!.minY, r1["clip.mp4"]!.minY,
                             "the box moved down under the inserted line")
    }

    func testMediaDeletedFromTheDocumentLeavesTheRects() {
        // What stops playback: a player whose src is absent from the new frame's
        // rects has been edited away, and the host removes it.
        let frame = docView([row([mkRun("just prose")])])
        let rects = EditorLayout(frame, theme: theme, wrapWidth: 400).mediaRects(theme: theme)
        XCTAssertNil(rects["clip.mp4"])
        XCTAssertTrue(rects.isEmpty)
    }

    func testMediaRectsMatchWhereTheBoxIsDrawn() {
        // The rect a player is installed at must be the one the still was drawn
        // into, or the player would sit off the block it belongs to.
        let frame = docView([row([mkRun("🎬 clip")])],
                            media: [mkMedia("clip.mp4", kind: .video, startRow: 0, endRow: 1)])
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400)
        let rl = layout.rows[0]
        let drawn = rl.media!.rect(top: rl.mediaTop,
                                   left: theme.padding.left + rl.shaped.prefixWidth)
        XCTAssertEqual(layout.mediaRects(theme: theme)["clip.mp4"], drawn)
    }

    // MARK: resolving a src

    func testResolveJoinsARelativePathToTheDocumentDirectory() {
        let store = MediaStore(baseURL: URL(fileURLWithPath: "/docs/notes/", isDirectory: true))
        XCTAssertEqual(store.resolve("img/cat.png")?.path, "/docs/notes/img/cat.png")
    }

    func testResolveTakesAnAbsolutePathAsItIs() {
        let store = MediaStore(baseURL: URL(fileURLWithPath: "/docs/", isDirectory: true))
        XCTAssertEqual(store.resolve("/tmp/cat.png")?.path, "/tmp/cat.png")
    }

    func testResolveDeclinesWhatThisLoaderCannotRead() {
        let store = MediaStore(baseURL: URL(fileURLWithPath: "/docs/", isDirectory: true))
        // Remote and inline sources need an async fetch or a decoder this
        // synchronous, local-file loader deliberately doesn't have.
        XCTAssertNil(store.resolve("https://example.com/cat.png"))
        XCTAssertNil(store.resolve("//cdn.example.com/cat.png"))
        XCTAssertNil(store.resolve("data:image/png;base64,iVBORw0KGgo="))
        XCTAssertNil(store.resolve("   "))
    }

    func testResolveDeclinesARelativePathWithNoDocumentDirectory() {
        // An untitled buffer has nothing for `./cat.png` to be relative *to*.
        XCTAssertNil(MediaStore(baseURL: nil).resolve("cat.png"))
    }

    // MARK: data: URIs — decoded here, no host, no network

    /// A 1×1 PNG as a `data:` URI, the smallest real thing to decode.
    private var dotDataURI: String {
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ"
            + "AAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
    }

    func testADataURIDecodesWithoutAHostOrABaseDirectory() {
        // The point of handling these natively: a self-contained document needs
        // neither a document directory nor anything from the app around it.
        let store = MediaStore(baseURL: nil)
        let still = store.still(for: mkMedia(dotDataURI))
        XCTAssertNotNil(still)
        XCTAssertEqual(still?.width, 1)
    }

    func testADataURIIsNeverHandedToTheHost() {
        var asked: [String] = []
        let store = MediaStore()
        store.onResolveMedia = { src, done in asked.append(src); done(nil) }
        _ = store.still(for: mkMedia(dotDataURI))
        XCTAssertTrue(asked.isEmpty, "it carries its own bytes; nothing to resolve")
    }

    func testADataURIPayloadMayBeWrapped() {
        // Base64 in a document is often line-wrapped, and whitespace is fatal to
        // the decoder — so it's stripped rather than the document being rejected.
        let wrapped = dotDataURI.replacingOccurrences(of: "base64,", with: "base64,\n  ")
        XCTAssertNotNil(MediaStore.decodeDataURI(wrapped))
    }

    func testAMalformedDataURIIsJustNoPicture() {
        XCTAssertNil(MediaStore.decodeDataURI("data:image/png;base64"), "no comma")
        XCTAssertNil(MediaStore.decodeDataURI("https://example.com/a.png"))
        XCTAssertNil(MediaStore(baseURL: nil).still(for: mkMedia("data:image/png;base64,!!!!")))
    }

    func testADataURIIsNotPlayable() {
        // Nothing can stream from a string; playing one would mean writing the
        // whole movie to a file first, for a spelling nobody uses for video.
        XCTAssertNil(MediaStore().playableURL(for: dotDataURI))
    }

    // MARK: the host resolver

    func testAnUnreadableSourceIsHandedToTheHostOnceAndCached() {
        var asked = 0
        let store = MediaStore()
        store.onResolveMedia = { _, done in asked += 1; done(nil) }
        let remote = mkMedia("https://example.com/cat.png")
        _ = store.still(for: remote)
        _ = store.still(for: remote)
        _ = store.still(for: remote)
        XCTAssertEqual(asked, 1, "a declined source is remembered, not re-asked every frame")
    }

    func testWithNoHostAnUnreadableSourceIsSimplyNoPicture() {
        let store = MediaStore()
        XCTAssertNil(store.still(for: mkMedia("https://example.com/cat.png")))
        XCTAssertFalse(store.isResolving("https://example.com/cat.png"))
    }

    func testAResolvedFileIsDecodedAndAnnounced() throws {
        let dir = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent("dot.png")
        try onePixelPNG().write(to: file)

        var announced: [String] = []
        let store = MediaStore()
        store.onLoaded = { announced.append($0) }
        // A host that answers immediately — the completion runs inline, so no
        // expectation/wait is needed and the test stays synchronous.
        store.onResolveMedia = { _, done in done(file) }

        let remote = mkMedia("https://example.com/cat.png")
        XCTAssertNil(store.still(for: remote), "nothing to draw on the first pass")
        XCTAssertEqual(announced, ["https://example.com/cat.png"],
                       "the view is told to repaint when the answer lands")
        XCTAssertNotNil(store.still(for: remote), "and now it has a picture")
    }

    func testAResolvedFileIsAlsoWhatPlaybackStreamsFrom() throws {
        // Why the host answers with a file rather than with bytes: the same
        // answer serves the still *and* the player.
        let dir = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent("clip.mp4")
        try Data([0, 1, 2, 3]).write(to: file)

        let store = MediaStore()
        store.onResolveMedia = { _, done in done(file) }
        XCTAssertNil(store.playableURL(for: "https://example.com/clip.mp4"),
                     "not yet — the host has only just been asked")
        XCTAssertEqual(store.playableURL(for: "https://example.com/clip.mp4"), file)
    }

    func testASourceInFlightReportsItself() {
        // So a tap can say "it's coming" rather than falling through as if the
        // reader had hit nothing.
        var pending: ((URL?) -> Void)?
        let store = MediaStore()
        store.onResolveMedia = { _, done in pending = done }
        _ = store.still(for: mkMedia("https://example.com/cat.png"))
        XCTAssertTrue(store.isResolving("https://example.com/cat.png"))
        pending?(nil)
        XCTAssertFalse(store.isResolving("https://example.com/cat.png"), "settled")
    }

    func testAnAnswerArrivingAfterAFlushIsDropped() throws {
        // The document was swapped (or its directory repointed) while the host
        // was still fetching; that answer belongs to a document that is gone.
        let dir = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent("dot.png")
        try onePixelPNG().write(to: file)

        var pending: ((URL?) -> Void)?
        var announced: [String] = []
        let store = MediaStore()
        store.onLoaded = { announced.append($0) }
        store.onResolveMedia = { _, done in pending = done }

        _ = store.still(for: mkMedia("https://example.com/cat.png"))
        store.flush()
        pending?(file)          // the late answer
        XCTAssertTrue(announced.isEmpty, "no repaint for a document that is gone")
        XCTAssertFalse(store.isResolving("https://example.com/cat.png"))
    }

    private func makeTempDir() throws -> URL {
        let dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("leaf-media-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private func onePixelPNG() -> Data {
        Data(base64Encoded: """
            iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==
            """)!
    }

    func testStillIsNilForAudioAndForAPosterlessVideo() {
        let store = MediaStore(baseURL: URL(fileURLWithPath: "/docs/", isDirectory: true))
        XCTAssertNil(store.still(for: mkMedia("take.mp3", kind: .audio)))
        XCTAssertNil(store.still(for: mkMedia("clip.mp4", kind: .video)))
    }

    func testStillLoadsARealImageAndCachesTheFailureOfAMissingOne() throws {
        let dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("leaf-media-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        // A 1x1 PNG, so the loader has a real file to decode.
        let png = Data(base64Encoded: """
            iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==
            """)!
        try png.write(to: dir.appendingPathComponent("dot.png"))

        let store = MediaStore(baseURL: dir)
        let loaded = store.still(for: mkMedia("dot.png"))
        XCTAssertNotNil(loaded, "a real PNG decodes")
        XCTAssertEqual(loaded?.width, 1)

        // A missing file answers nil — and, cached as such, keeps answering nil
        // without going back to disk on every redraw.
        XCTAssertNil(store.still(for: mkMedia("gone.png")))
        XCTAssertNil(store.still(for: mkMedia("gone.png")))
    }

    func testFlushDropsLoadedStills() throws {
        let dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("leaf-media-flush-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let png = Data(base64Encoded: """
            iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==
            """)!
        try png.write(to: dir.appendingPathComponent("dot.png"))

        let store = MediaStore(baseURL: dir)
        XCTAssertNotNil(store.still(for: mkMedia("dot.png")))
        // Delete the file, then flush: a stale hit would still answer non-nil.
        try FileManager.default.removeItem(at: dir.appendingPathComponent("dot.png"))
        store.flush()
        XCTAssertNil(store.still(for: mkMedia("dot.png")),
                     "flush must drop the cache, not just mark it stale")
    }
}
