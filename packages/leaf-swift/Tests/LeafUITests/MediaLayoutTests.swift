//  MediaLayoutTests.swift
//
//  The geometry of a block media box: how it fits a picture, how it collapses
//  core's reserved rows onto one box, and what it resolves a `src` to.
//
//  No `MediaStore` is handed to the layout here, so every box lays out at its
//  no-picture size — which is the point: the collapsing and the row bookkeeping
//  are what these test, and neither should need a file on disk to exercise.

import CoreGraphics
import ImageIO
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
        // An image has nothing to *play*. Whether a click on one is worth acting
        // on depends on what has loaded into its box, which only a view can see —
        // so geometry answers `mediaBox` for it and `playableMedia` not at all.
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

        // The superset does answer for the image — an empty picture box is what
        // the reader clicks to ask the host for it.
        XCTAssertEqual(layout.mediaBox(at: inImage, theme: theme)?.src, "cat.png")
        XCTAssertEqual(layout.mediaBox(at: inVideo, theme: theme)?.src, "clip.mp4")
    }

    func testAPointOutsideEveryBoxHitsNothing() {
        let frame = docView([row([mkRun("🎬 clip")])],
                            media: [mkMedia("clip.mp4", kind: .video, startRow: 0, endRow: 1)])
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400)
        XCTAssertNil(layout.playableMedia(at: CGPoint(x: 5000, y: 5000), theme: theme))
    }

    // MARK: the caret around a picture

    /// A document that ends with a picture — the shape the caret bugs show up in.
    private func pictureLast() -> EditorLayout {
        let frame = docView(
            [row([mkRun("prose above")]), row([mkRun("🖼 a cat")])],
            media: [mkMedia("cat.png", startRow: 1, endRow: 2)]
        )
        return EditorLayout(frame, theme: theme, wrapWidth: 400)
    }

    func testATapBelowATrailingPictureLandsAfterItNotBeforeIt() {
        // Regression: every point below the last row clamps onto it, and when
        // that row is a picture the label glyphs it's painted over answered the
        // hit — putting the caret in *front* of the photo (drawn as a bar down
        // its left edge) for a tap on the empty page under it.
        let layout = pictureLast()
        let label = layout.rows[1].attributed.length
        let (row, ch) = layout.hit(CGPoint(x: theme.padding.left + 4, y: 99_999), theme: theme)
        XCTAssertEqual(row, 1)
        XCTAssertEqual(ch, label, "past the label's last glyph — core's stop after the image")
    }

    func testTheTopHalfOfAPictureIsStillInFrontOfIt() {
        // The other home: a tap on the picture's upper half means "before this".
        let layout = pictureLast()
        let box = layout.rows[1]
        let top = box.mediaTop + MediaMetrics.gap + 2
        let (row, ch) = layout.hit(CGPoint(x: theme.padding.left + 40, y: top), theme: theme)
        XCTAssertEqual(row, 1)
        XCTAssertEqual(ch, 0)
    }

    func testTheCaretRidesTheBoxEdgesAndIsAsTallAsThePicture() throws {
        let layout = pictureLast()
        let rl = layout.rows[1]
        let drawn = rl.media!.rect(top: rl.mediaTop, left: theme.padding.left + rl.shaped.prefixWidth)

        let before = try XCTUnwrap(layout.rect(row: 1, ch: 0, theme: theme))
        XCTAssertEqual(before.minX, drawn.minX, accuracy: 0.5, "at the picture's leading edge")
        XCTAssertEqual(before.minY, drawn.minY, accuracy: 0.5)
        XCTAssertEqual(before.height, drawn.height, accuracy: 0.5, "as tall as the box")

        let after = try XCTUnwrap(layout.rect(row: 1, ch: rl.attributed.length, theme: theme))
        XCTAssertEqual(after.maxX, drawn.maxX, accuracy: 0.5, "at its trailing edge")
        XCTAssertEqual(after.minY, drawn.minY, accuracy: 0.5)
        XCTAssertEqual(after.height, drawn.height, accuracy: 0.5)
        XCTAssertGreaterThan(after.minX, before.minX)
    }

    func testAReservedRowBelowThePictureIsTheCaretHomePastIt() throws {
        // The blank rows core reserves under a measured picture carry no glyphs,
        // so they're only ever the stop past it — and their caret must still draw
        // on the box, not at the origin of a zero-height row.
        let frame = docView(
            [row([mkRun("🖼 a cat")]), row([])],
            media: [mkMedia("cat.png", startRow: 0, endRow: 2)]
        )
        let layout = EditorLayout(frame, theme: theme, wrapWidth: 400)
        let rl = layout.rows[0]
        let drawn = rl.media!.rect(top: rl.mediaTop, left: theme.padding.left)
        let caret = try XCTUnwrap(layout.rect(row: 1, ch: 0, theme: theme))
        XCTAssertEqual(caret.maxX, drawn.maxX, accuracy: 0.5)
        XCTAssertEqual(caret.height, drawn.height, accuracy: 0.5)
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

    // MARK: a local path that isn't here yet

    func testALocalPathWithNoBytesYetIsOfferedToTheHost() throws {
        // The synced-vault case: `img/cat.png` composes to a perfectly good URL
        // and there is nothing at it, because the provider hasn't materialized
        // the placeholder. That is the host's to fetch, not a decode failure to
        // remember — so it goes to the host like any source this loader can't
        // read, and the answer draws.
        let dir = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let elsewhere = dir.appendingPathComponent("materialized.png")
        try onePixelPNG().write(to: elsewhere)

        var asked: [String] = []
        let store = MediaStore(baseURL: dir)
        store.onResolveMedia = { src, done in asked.append(src); done(elsewhere) }

        XCTAssertNil(store.still(for: mkMedia("img/cat.png")), "nothing to draw on the first pass")
        XCTAssertEqual(asked, ["img/cat.png"], "asked with the src as written")
        XCTAssertNotNil(store.still(for: mkMedia("img/cat.png")), "and now it has a picture")
    }

    func testALocalFileThatIsHereIsNeverOfferedToTheHost() throws {
        // The common case must not acquire a round trip through the app.
        let dir = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        try onePixelPNG().write(to: dir.appendingPathComponent("dot.png"))

        var asked = 0
        let store = MediaStore(baseURL: dir)
        store.onResolveMedia = { _, done in asked += 1; done(nil) }
        XCTAssertNotNil(store.still(for: mkMedia("dot.png")))
        XCTAssertEqual(asked, 0)
    }

    func testALocalVideoIsPlayableWithoutTheHostEvenThoughItDecodesToNoStill() throws {
        // Readability is the test, not a successful decode: `load` will never
        // make a CGImage out of an mp4, and the file URL is exactly what
        // playback needs. Guarding on the decode would send every local video
        // to the host.
        let dir = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let clip = dir.appendingPathComponent("clip.mp4")
        try Data([0, 1, 2, 3]).write(to: clip)

        var asked = 0
        let store = MediaStore(baseURL: dir)
        store.onResolveMedia = { _, done in asked += 1; done(nil) }
        XCTAssertEqual(store.playableURL(for: "clip.mp4"), clip)
        XCTAssertEqual(asked, 0)
    }

    // MARK: forgetting a decline

    func testForgetLetsADeclinedSourceBeAskedAgain() throws {
        // What `reloadMedia` is for: the app declined on open, the reader said
        // load it anyway, and the refusal has to be reconsidered.
        let dir = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent("dot.png")
        try onePixelPNG().write(to: file)

        var allow = false
        var asked = 0
        let store = MediaStore()
        store.onResolveMedia = { _, done in asked += 1; done(allow ? file : nil) }

        let remote = mkMedia("https://example.com/cat.png")
        XCTAssertNil(store.still(for: remote), "declined")
        XCTAssertNil(store.still(for: remote))
        XCTAssertEqual(asked, 1, "and the decline is cached")

        allow = true
        store.forget("https://example.com/cat.png")
        XCTAssertNil(store.still(for: remote), "asked again — and, as on any first pass, nothing yet")
        XCTAssertEqual(asked, 2)
        XCTAssertNotNil(store.still(for: remote), "and this time the answer was a file")
    }

    func testForgetLeavesASourceInFlightAlone() {
        // Forgetting a pending entry would send the host after the same bytes a
        // second time, and the first answer would still be on its way.
        var asked = 0
        let store = MediaStore()
        store.onResolveMedia = { _, done in asked += 1; _ = done }
        let remote = mkMedia("https://example.com/cat.png")
        _ = store.still(for: remote)
        store.forget("https://example.com/cat.png")
        _ = store.still(for: remote)
        XCTAssertEqual(asked, 1)
        XCTAssertTrue(store.isResolving("https://example.com/cat.png"))
    }

    func testForgetEverythingIsAFlush() throws {
        let dir = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        try onePixelPNG().write(to: dir.appendingPathComponent("dot.png"))
        let store = MediaStore(baseURL: dir)
        XCTAssertNotNil(store.still(for: mkMedia("dot.png")))
        try FileManager.default.removeItem(at: dir.appendingPathComponent("dot.png"))
        store.forget(nil)
        XCTAssertNil(store.still(for: mkMedia("dot.png")))
    }

    // MARK: the way up a photo says it goes

    func testATaggedPhotoIsTurnedTheWayUpItSaysItGoes() throws {
        // A camera stores the sensor's pixels and a tag saying how it was held,
        // so a photo taken upside down is stored upside down. Decoding the
        // pixels and dropping the tag draws it that way, which is what a phone
        // showed for every picture it took in any orientation but one.
        let dir = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        try cornerMarkedJPEG(width: 32, height: 32, orientation: .down)
            .write(to: dir.appendingPathComponent("photo.jpg"))

        let still = try XCTUnwrap(MediaStore(baseURL: dir).still(for: mkMedia("photo.jpg")))
        XCTAssertFalse(isRed(still, x: 8, y: 8), "the stored corner is not where it is drawn")
        XCTAssertTrue(isRed(still, x: 24, y: 24), "half a turn puts it opposite")
    }

    func testAQuarterTurnSwapsTheSizeTheBoxIsMeasuredFrom() throws {
        // Why this isn't only a drawing nicety: `MediaLayout` measures the box
        // from the picture's own dimensions, and a quarter turn exchanges them.
        // A portrait photo laid out landscape is the wrong shape whatever is
        // then painted into it.
        let dir = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        try cornerMarkedJPEG(width: 400, height: 200, orientation: .right)
            .write(to: dir.appendingPathComponent("photo.jpg"))

        let still = try XCTUnwrap(MediaStore(baseURL: dir).still(for: mkMedia("photo.jpg")))
        XCTAssertEqual(still.width, 200)
        XCTAssertEqual(still.height, 400, "and at its own resolution, not a thumbnail's")

        let box = MediaLayout(mkMedia("photo.jpg"), still: still, contentWidth: 600, theme: theme)
        XCTAssertGreaterThan(box.size.height, box.size.width, "laid out as the portrait it is")
    }

    func testAnUntaggedPictureIsLeftExactlyAsItIs() throws {
        // The common case — a screenshot, a drawing, anything not off a camera.
        // It must not acquire a turn, and it takes the cheap decode.
        let dir = try makeTempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        try cornerMarkedJPEG(width: 32, height: 32, orientation: nil)
            .write(to: dir.appendingPathComponent("shot.jpg"))

        let still = try XCTUnwrap(MediaStore(baseURL: dir).still(for: mkMedia("shot.jpg")))
        XCTAssertTrue(isRed(still, x: 8, y: 8), "the corner is where it was stored")
    }

    /// A JPEG with its top-left quarter red and the rest white, tagged with
    /// `orientation` — or with no tag at all, which is how everything that isn't
    /// a photograph arrives. The mark is a quarter rather than a pixel because
    /// JPEG is lossy and a lone pixel would be smeared into its neighbours.
    private func cornerMarkedJPEG(
        width: Int, height: Int, orientation: CGImagePropertyOrientation?
    ) throws -> Data {
        let context = try XCTUnwrap(CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue))
        context.setFillColor(gray: 1, alpha: 1)
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        context.setFillColor(red: 1, green: 0, blue: 0, alpha: 1)
        // Quartz counts up from the bottom, so the *top* half is the far end.
        context.fill(CGRect(x: 0, y: height / 2, width: width / 2, height: height / 2))
        let image = try XCTUnwrap(context.makeImage())

        let data = NSMutableData()
        let out = try XCTUnwrap(CGImageDestinationCreateWithData(
            data, "public.jpeg" as CFString, 1, nil))
        var properties: [CFString: Any] = [kCGImageDestinationLossyCompressionQuality: 1.0]
        if let orientation { properties[kCGImagePropertyOrientation] = orientation.rawValue }
        CGImageDestinationAddImage(out, image, properties as CFDictionary)
        XCTAssertTrue(CGImageDestinationFinalize(out))
        return data as Data
    }

    /// Whether the pixel at (`x`, `y`), counted from the top-left the way a
    /// picture is read, is the red mark rather than the white field. Loose about
    /// the exact values, since JPEG does not hand back what it was given.
    private func isRed(_ image: CGImage, x: Int, y: Int) -> Bool {
        var pixel = [UInt8](repeating: 0, count: 4)
        let drawn: Bool = pixel.withUnsafeMutableBytes { raw in
            guard let context = CGContext(
                data: raw.baseAddress, width: 1, height: 1, bitsPerComponent: 8, bytesPerRow: 4,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return false }
            // Slide the picture so the pixel wanted lands in the one-pixel
            // window, remembering that Quartz's y runs the other way.
            context.draw(image, in: CGRect(x: -x, y: y - image.height + 1,
                                           width: image.width, height: image.height))
            return true
        }
        guard drawn else { return false }
        return pixel[0] > 200 && pixel[1] < 80 && pixel[2] < 80
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

        // A missing file answers nil — and with no host to offer it to, that is
        // cached, so it keeps answering nil without going back to disk on every
        // redraw.
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
