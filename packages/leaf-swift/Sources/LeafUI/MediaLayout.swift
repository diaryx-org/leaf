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
        } else {
            // No picture to show: a chip wide enough for its label, at one line of
            // text plus its padding — the same shape a video's poster-less box and
            // a broken image's frame both want.
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
    /// The play badge's diameter.
    static let badge: CGFloat = 44
    /// The box's corner rounding.
    static let corner: CGFloat = 6
}

/// Loads and caches the stills a media box draws — an image's own file, or a
/// video's poster frame.
///
/// One entry per resolved URL, and a *failure is cached too* (`.some(nil)`), so a
/// broken path is attempted once rather than on every frame. That matters more
/// here than in the terminal: an AppKit view redraws on every caret blink.
///
/// Deliberately synchronous and local-file-only, matching leaf-gpui's loader. A
/// remote URL resolves to `nil` and draws as a chip, because loading it would
/// mean an async fetch and a repaint on completion — worth doing, but it belongs
/// with the same work that gives video real playback rather than bolted onto a
/// draw call.
final class MediaStore {
    /// The document's own directory, which a relative `src` resolves against.
    /// Core holds no I/O and no path context, so the host supplies this; `nil`
    /// for an untitled buffer, where a relative path has nothing to resolve to.
    var baseURL: URL?

    private var cache: [URL: CGImage?] = [:]

    init(baseURL: URL? = nil) {
        self.baseURL = baseURL
    }

    /// Drop every loaded still — for a document swap, or a base directory change
    /// that repoints every relative path.
    func flush() {
        cache.removeAll()
    }

    /// The still for `media` under the current base directory, or `nil` when
    /// there is nothing to draw.
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
        guard !source.isEmpty, let url = resolve(source) else { return nil }
        if let hit = cache[url] { return hit }
        let loaded = MediaStore.load(url)
        cache[url] = loaded
        return loaded
    }

    /// Resolve a `src` to a readable local file URL, or `nil` when it isn't one
    /// this loader handles — a remote URL, a `data:` URI, or a relative path with
    /// no document directory to resolve against.
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

    /// Decode an image file, or `nil` on any failure — a missing file, or a
    /// format ImageIO doesn't read.
    private static func load(_ url: URL) -> CGImage? {
        guard let src = CGImageSourceCreateWithURL(url as CFURL, nil),
              CGImageSourceGetCount(src) > 0 else { return nil }
        return CGImageSourceCreateImageAtIndex(src, 0, nil)
    }
}
