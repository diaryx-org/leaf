//  MediaLayout.swift
//
//  Block-level images, video, and audio: where they sit and what gets drawn
//  there. The Apple peer of `leaf-gpui`'s image painting and `leaf-ratatui`'s
//  `image.rs`, and the structural sibling of `TableLayout` — core reserves a
//  placeholder row carrying a `MediaView`, and this replaces it with a real box.
//
//  ## What this draws, and what it doesn't
//
//  A still picture is drawn here, into the same `CGContext` as the text: an
//  image's own file, or a `<video poster="…">`'s frame. That covers every case a
//  glance at the document should answer.
//
//  It deliberately stops short of *playback*. A live `AVPlayerView` is a subview,
//  not a draw call — it needs its own lifecycle, scroll-synced positioning, and
//  first-responder negotiation with a text view that owns the caret, and on iOS
//  an `AVPlayerViewController` needs a parent view controller LeafUI doesn't
//  have. So a video draws its poster (or a labelled chip) with a play badge, and
//  activating it calls the host's `onOpenMedia` — the same division of labour
//  `onOpenLink` already uses, and the same one the package README states: LeafUI
//  renders the editing surface and leaves windows, files, and system UI to the
//  host, which *does* have a view controller to present a player from.
//
//  ## Sizing
//
//  Core reserves rows and asks how many it should have been (`set_media_rows`),
//  the way a terminal answers in character cells. A proportional GUI doesn't play
//  that game — like leaf-gpui it lays the box out in points and leaves core's
//  reservation at one row, so nothing here calls back with a height.

import CoreGraphics
import CoreText
import Foundation
import ImageIO
import LeafFFI

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

#if canImport(AVFoundation)
import AVFoundation
#endif

/// The box one block media occupies, and what to paint in it.
struct MediaLayout {
    /// The media this stands for — kind, URL, alt, poster.
    let media: MediaView
    /// The drawn box's size in points, including the frame but not the outer gap.
    let size: CGSize
    /// The still to paint, already loaded: the picture itself, or a video's
    /// poster frame. `nil` when there is nothing to draw — audio, a poster-less
    /// video, or a file that wouldn't load — and the box then shows a labelled
    /// chip instead.
    let still: CGImage?
    /// Whether the box should carry a play badge (video and audio; never a still
    /// picture), so it reads as something to start rather than something to look
    /// at.
    var showsPlayBadge: Bool { media.kind != .image }
    /// Whether this is a placeholder for media that should have had a picture but
    /// didn't load — drawn with a dashed frame, so a broken path is legible
    /// rather than silently blank.
    var isBroken: Bool { media.kind == .image && still == nil }

    /// The total height the row reserves: the box plus the breathing room above
    /// and below that keeps it from crowding the prose.
    var height: CGFloat { size.height + MediaMetrics.gap * 2 }

    /// Lay `media` out into a column `contentWidth` points wide.
    ///
    /// A picture (or poster) is fitted to its own aspect ratio, never enlarged
    /// past its natural size — an upscaled 16pt icon spanning the text column
    /// looks like a bug — and capped in height so one tall image can't push a
    /// screen of text away. Audio has no aspect ratio at all, so it gets a fixed
    /// control height and the width it needs, whichever is smaller.
    init(_ media: MediaView, still: CGImage?, contentWidth: CGFloat, theme: EditorTheme) {
        self.media = media
        self.still = still

        let maxW = max(MediaMetrics.minWidth, contentWidth)
        let maxH = MediaMetrics.maxHeight

        if media.kind == .audio {
            self.size = CGSize(width: min(maxW, MediaMetrics.audioWidth),
                               height: MediaMetrics.audioHeight)
        } else if let img = still {
            let natural = CGSize(width: CGFloat(img.width), height: CGFloat(img.height))
            self.size = MediaLayout.fit(natural, maxWidth: maxW, maxHeight: maxH)
        } else if media.kind == .video {
            // A video with no poster still *is* a picture — we just haven't been
            // handed one to measure. Reserving a chip would be right for something
            // with nothing to show, and wrong here: the moment it plays, the
            // player has to letterbox a whole movie into a strip the height of a
            // line of text. So take the default video shape and let the picture
            // fill it. A real aspect would need the asset's track, which means
            // opening the movie — playback's job, not layout's.
            self.size = MediaLayout.fit(MediaMetrics.videoAspect, maxWidth: maxW, maxHeight: maxH)
        } else {
            // Nothing to show and nothing expected — an image whose file wouldn't
            // load. A chip wide enough for its label, one line of text tall.
            self.size = CGSize(width: min(maxW, MediaMetrics.chipWidth),
                               height: max(theme.lineHeight, MediaMetrics.audioHeight))
        }
    }

    /// Fit `natural` inside the box without distorting it or scaling it up.
    static func fit(_ natural: CGSize, maxWidth: CGFloat, maxHeight: CGFloat) -> CGSize {
        guard natural.width > 0, natural.height > 0 else {
            return CGSize(width: maxWidth, height: maxHeight)
        }
        // `1` keeps a small picture at its own size; the other two are the box.
        let scale = min(1, maxWidth / natural.width, maxHeight / natural.height)
        return CGSize(width: (natural.width * scale).rounded(),
                      height: (natural.height * scale).rounded())
    }

    /// The box in view coordinates, given the top of the media's reserved row and
    /// the text column's left edge.
    func rect(top: CGFloat, left: CGFloat) -> CGRect {
        CGRect(x: left, y: top + MediaMetrics.gap, width: size.width, height: size.height)
    }

    /// The text shown when there's no picture — a video or audio's own name, so
    /// the chip says which file it stands for rather than just "video".
    var chipLabel: String {
        if !media.alt.isEmpty { return media.alt }
        let name = (media.src as NSString).lastPathComponent
        return name.isEmpty ? media.src : name
    }
}

/// Fixed geometry for a media box. Not on `EditorTheme`: these are the media
/// equivalents of `TableMetrics`, describing the shape of the thing rather than
/// the document's typography, and a host retheming its fonts has no reason to
/// restate them.
enum MediaMetrics {
    /// Breathing room above and below a media box.
    static let gap: CGFloat = 6
    /// The tallest a picture is drawn, so one image can't fill the viewport.
    /// Mirrors leaf-gpui's `IMAGE_MAX_H` and leaf-ratatui's `MAX_IMAGE_ROWS`.
    static let maxHeight: CGFloat = 420
    /// The narrowest column a box will lay out into, so a very narrow view still
    /// draws something rather than collapsing to nothing.
    static let minWidth: CGFloat = 80
    /// An audio transport: no picture, so a fixed control height.
    static let audioWidth: CGFloat = 360
    static let audioHeight: CGFloat = 44
    /// A chip standing in for media with no picture to show.
    static let chipWidth: CGFloat = 280
    /// The shape a video is reserved at when it has no poster to measure — the
    /// 16:9 nearly all video is, scaled to the column by the usual fitting. Only
    /// the ratio matters; the absolute numbers are large enough that `fit` scales
    /// down to the column rather than clamping at 1.
    static let videoAspect = CGSize(width: 1600, height: 900)
    /// The play badge's diameter.
    static let badge: CGFloat = 44
    /// The box's corner rounding.
    static let corner: CGFloat = 6
}

/// Loads and caches the stills a media box draws — an image's own file, a
/// video's poster frame, or a `data:` URI's inline bytes — and holds whatever
/// the host has resolved on its behalf.
///
/// One entry per *source string*, and a failure is cached too, so a broken path
/// is attempted once rather than on every frame. That matters more here than in
/// the terminal: an AppKit view redraws on every caret blink.
///
/// ## Three kinds of source, and only two it reads itself
///
///   * **A local path** — read straight off disk, synchronously, as before.
///   * **A `data:` URI** — decoded inline. It carries its own bytes, so there is
///     nothing to fetch and no reason to involve anyone; this is what makes a
///     self-contained single-file document render.
///   * **Anything else** (`https:`, or a scheme only the host understands) —
///     handed to [`onResolveMedia`]. **LeafUI never touches the network.**
///     Opening a document that silently fetches from a server discloses the
///     reader's address and the moment they opened it, and that is the host's
///     decision to make, not an editor's. The host fetches (or declines), caches
///     wherever it likes, and answers with a local file URL.
///
/// Answering with a *file* rather than with bytes is deliberate: the same
/// answer then serves both uses, decoding through the path a local picture
/// already takes and giving `AVPlayer` something it can stream from disk. Bytes
/// would yield a still and leave playback needing the whole movie in memory.
///
/// [`onResolveMedia`]: MediaStore.onResolveMedia
final class MediaStore {
    /// The document's own directory, which a relative `src` resolves against.
    /// Core holds no I/O and no path context, so the host supplies this; `nil`
    /// for an untitled buffer, where a relative path has nothing to resolve to.
    var baseURL: URL?

    /// Asks the host to turn a source this loader can't read — a remote URL, or
    /// any scheme only the host knows — into a local file it can.
    ///
    /// Called on the main thread with the raw `src`, at most once per source per
    /// document. The host answers with a file URL, or `nil` to decline (which is
    /// cached, so declining is cheap and permanent until the document reloads).
    /// It may answer immediately or much later; the completion is safe to call
    /// from any thread.
    var onResolveMedia: ((String, @escaping (URL?) -> Void) -> Void)?

    /// Fired with a source whose resolution just completed, so the view can
    /// repaint — and start playing, if that source is what the reader tapped.
    /// Always called on the main thread.
    var onLoaded: ((String) -> Void)?

    /// What is known about one source.
    private enum Entry {
        /// Handed to the host; nothing to draw until it answers.
        case pending
        /// Settled. `file` is the local file it lives in, if any (what playback
        /// needs); `still` is the decoded picture, if it is one.
        case ready(file: URL?, still: CGImage?)
    }

    private var entries: [String: Entry] = [:]
    /// Bumped by `flush`, so an answer arriving for a document that has since
    /// been swapped or repointed is dropped instead of populating a stale cache.
    private var generation = 0

    init(baseURL: URL? = nil) {
        self.baseURL = baseURL
    }

    /// Drop everything loaded — for a document swap, or a base directory change
    /// that repoints every relative path. Answers still in flight are discarded
    /// when they arrive.
    func flush() {
        entries.removeAll()
        generation &+= 1
    }

    /// The still for `media`, or `nil` when there is nothing to draw *yet* —
    /// audio, a poster-less video, a source that failed, or one the host is
    /// still resolving. The box draws its labelled chip meanwhile.
    func still(for media: MediaView) -> CGImage? {
        // An image is its own still; a video's is its poster. Audio has none, and
        // a video without a poster has none either — core already leaves `poster`
        // empty rather than inventing one, and decoding a frame out of the movie
        // is playback's job, not a draw call's.
        let source: String
        switch media.kind {
        case .image: source = media.src
        case .video: source = media.poster
        case .audio: return nil
        }
        guard !source.isEmpty else { return nil }
        switch settle(source) {
        case .pending: return nil
        case .ready(_, let still): return still
        }
    }

    /// The local file `src` can be played from, or `nil` when there isn't one
    /// yet. A local path answers immediately; anything else answers only once
    /// the host has resolved it (and `onLoaded` will have fired when it did).
    func playableURL(for src: String) -> URL? {
        let trimmed = src.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        // A `data:` movie would have to be written to a file before anything
        // could play it, which is a copy of a whole video for a spelling nobody
        // uses. Stills are the case that spelling is actually for.
        guard !trimmed.lowercased().hasPrefix("data:") else { return nil }
        switch settle(trimmed) {
        case .pending: return nil
        case .ready(let file, _): return file
        }
    }

    /// Whether `src` is waiting on the host — so a tap can say "it's coming"
    /// rather than falling through as if nothing happened.
    func isResolving(_ src: String) -> Bool {
        if case .pending = entries[src.trimmingCharacters(in: .whitespacesAndNewlines)] {
            return true
        }
        return false
    }

    /// The cache entry for `source`, starting whatever work it needs.
    private func settle(_ source: String) -> Entry {
        if let hit = entries[source] { return hit }

        // A `data:` URI carries its own bytes: decode and be done, no host, no
        // network, no file on disk for playback to point at.
        if source.lowercased().hasPrefix("data:") {
            let still = MediaStore.decodeDataURI(source).flatMap(MediaStore.decode)
            let entry = Entry.ready(file: nil, still: still)
            entries[source] = entry
            return entry
        }

        // A path this loader can read itself.
        if let url = resolve(source) {
            let entry = Entry.ready(file: url, still: MediaStore.load(url))
            entries[source] = entry
            return entry
        }

        // Anything else is the host's to answer — or, with no host hook, simply
        // unreadable, which is cached so it isn't re-asked every frame.
        guard let ask = onResolveMedia else {
            let entry = Entry.ready(file: nil, still: nil)
            entries[source] = entry
            return entry
        }
        entries[source] = .pending
        let token = generation
        ask(source) { [weak self] url in
            // The host may answer from anywhere; the cache and the repaint both
            // belong to the main thread.
            MediaStore.onMain {
                guard let self, self.generation == token else { return }
                let still = url.flatMap(MediaStore.load)
                self.entries[source] = .ready(file: url, still: still)
                self.onLoaded?(source)
            }
        }
        return .pending
    }

    /// Run `work` on the main thread, now if we are already on it — so a host
    /// that answers synchronously doesn't bounce through the run loop and make a
    /// cheap answer arrive a frame late.
    private static func onMain(_ work: @escaping () -> Void) {
        if Thread.isMainThread {
            work()
        } else {
            DispatchQueue.main.async(execute: work)
        }
    }

    /// Resolve a `src` to a readable local file URL, or `nil` when it isn't one
    /// this loader handles — a remote URL, a `data:` URI, or a relative path with
    /// no document directory to resolve against. Those go to the host instead.
    func resolve(_ src: String) -> URL? {
        let trimmed = src.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        let lower = trimmed.lowercased()
        if lower.hasPrefix("http://") || lower.hasPrefix("https://")
            || lower.hasPrefix("data:") || trimmed.hasPrefix("//") {
            return nil
        }
        if lower.hasPrefix("file://") { return URL(string: trimmed) }
        if trimmed.hasPrefix("/") { return URL(fileURLWithPath: trimmed) }
        guard let base = baseURL else { return nil }
        return URL(fileURLWithPath: trimmed, relativeTo: base).standardizedFileURL
    }

    /// The bytes of a `data:` URI, or `nil` if it isn't one or is malformed.
    ///
    /// `data:[<mediatype>][;base64],<data>`. The media type is ignored — ImageIO
    /// sniffs the real format from the bytes, and a document claiming `image/png`
    /// for a JPEG should still draw.
    static func decodeDataURI(_ src: String) -> Data? {
        let trimmed = src.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.lowercased().hasPrefix("data:"),
              let comma = trimmed.firstIndex(of: ",") else { return nil }
        let meta = trimmed[trimmed.index(trimmed.startIndex, offsetBy: 5)..<comma].lowercased()
        let payload = String(trimmed[trimmed.index(after: comma)...])
        if meta.contains("base64") {
            // Whitespace is legal inside a wrapped base64 payload and fatal to
            // the decoder, so strip it rather than reject the document.
            let packed = payload.filter { !$0.isWhitespace }
            return Data(base64Encoded: packed, options: [.ignoreUnknownCharacters])
        }
        return payload.removingPercentEncoding?.data(using: .utf8)
    }

    /// Decode an image file, or `nil` on any failure — a missing file, or a
    /// format ImageIO doesn't read.
    private static func load(_ url: URL) -> CGImage? {
        guard let src = CGImageSourceCreateWithURL(url as CFURL, nil),
              CGImageSourceGetCount(src) > 0 else { return nil }
        return CGImageSourceCreateImageAtIndex(src, 0, nil)
    }

    /// Decode image bytes already in memory — a `data:` URI's payload.
    private static func decode(_ data: Data) -> CGImage? {
        guard let src = CGImageSourceCreateWithData(data as CFData, nil),
              CGImageSourceGetCount(src) > 0 else { return nil }
        return CGImageSourceCreateImageAtIndex(src, 0, nil)
    }
}
