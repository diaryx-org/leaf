//  AudioStrip.swift
//
//  The player an audio box becomes when it is activated: a play button, the
//  file's name over a track of where playback is, and the time — drawn by
//  leaf, inside the box `MediaLayout` reserved and nowhere else.
//
//  Audio used to get AVKit's player, the same as video. AVKit's player is a
//  video player: on iOS its controls need far more than the 44 points an audio
//  box is, so they spilled out over the page — a QuickTime emblem the size of
//  the screen, full-screen and AirPlay buttons for a picture there isn't, a
//  scrubber drawn through the next paragraph — and on the Mac its inline bar
//  is a video's bar too. A recording needs three things, which fit in a line:
//  start and stop, where it is, and how long it runs.
//
//  The strip is the interactive surface, so a tap on it is the strip's and not
//  the text view's: the play button toggles, and the track — tap or drag —
//  is a seek. A drag that starts mostly up or down is left to the scroll
//  view, so a thumb landing on a recording still scrolls the page.

import AVFoundation
import CoreGraphics
import Foundation

#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

final class AudioStrip: LeafView {
    let player: AVPlayer
    private let name: String
    var theme: EditorTheme { didSet { redraw() } }

    private var timeObserver: Any?
    private var endObserver: NSObjectProtocol?
    private var statusObserver: NSKeyValueObservation?
    /// Where a drag on the track has put playback, ahead of the player
    /// catching up — so the knob follows the finger, not the seek.
    private var scrubbing: Double?

    init(player: AVPlayer, name: String, theme: EditorTheme) {
        self.player = player
        self.name = name
        self.theme = theme
        super.init(frame: .zero)
        #if canImport(UIKit)
        backgroundColor = .clear
        isOpaque = false
        contentMode = .redraw
        isAccessibilityElement = true
        accessibilityTraits = [.adjustable, .startsMediaSession]
        let tap = UITapGestureRecognizer(target: self, action: #selector(handleTap(_:)))
        addGestureRecognizer(tap)
        let pan = UIPanGestureRecognizer(target: self, action: #selector(handlePan(_:)))
        addGestureRecognizer(pan)
        #elseif canImport(AppKit)
        setAccessibilityElement(true)
        setAccessibilityRole(.button)
        #endif

        timeObserver = player.addPeriodicTimeObserver(
            forInterval: CMTime(seconds: 0.1, preferredTimescale: 600), queue: .main
        ) { [weak self] _ in self?.redraw() }
        endObserver = NotificationCenter.default.addObserver(
            forName: AVPlayerItem.didPlayToEndTimeNotification, object: player.currentItem, queue: .main
        ) { [weak self] _ in self?.redraw() }
        statusObserver = player.observe(\.timeControlStatus) { [weak self] _, _ in
            DispatchQueue.main.async { self?.redraw() }
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    /// Stop observing the player — called as the strip is taken down, so the
    /// periodic observer doesn't outlive the view it redraws.
    func tearDown() {
        if let timeObserver { player.removeTimeObserver(timeObserver) }
        timeObserver = nil
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        endObserver = nil
        statusObserver = nil
    }

    // MARK: transport

    private var duration: Double {
        let seconds = player.currentItem?.duration.seconds ?? 0
        return seconds.isFinite && seconds > 0 ? seconds : 0
    }

    private var position: Double {
        if let scrubbing { return scrubbing }
        let seconds = player.currentTime().seconds
        return seconds.isFinite ? max(0, seconds) : 0
    }

    private var isPlaying: Bool { player.timeControlStatus != .paused }

    /// Play or pause; a play at the end starts again from the beginning, since
    /// a recording that has finished has nothing after it to resume.
    func toggle() {
        if isPlaying {
            player.pause()
        } else {
            if duration > 0, position >= duration - 0.05 {
                player.seek(to: .zero)
            }
            player.play()
        }
        redraw()
    }

    private func seek(to seconds: Double) {
        let clamped = min(max(0, seconds), duration)
        player.seek(to: CMTime(seconds: clamped, preferredTimescale: 600),
                    toleranceBefore: .zero, toleranceAfter: .zero)
        redraw()
    }

    private func redraw() {
        #if canImport(UIKit)
        setNeedsDisplay()
        accessibilityValue = "\(Self.clock(position)) of \(Self.clock(duration))"
        #elseif canImport(AppKit)
        needsDisplay = true
        #endif
    }

    // MARK: geometry

    /// Where the unplayed chip's play badge was, so the button doesn't move
    /// under the finger that pressed it.
    private var button: CGRect {
        let d = min(MediaMetrics.badge, bounds.height - 8, bounds.width - 8)
        return CGRect(x: bounds.minX + 8, y: bounds.midY - d / 2, width: d, height: d)
    }

    private var timeAttributes: [NSAttributedString.Key: Any] {
        [.font: LeafFont.monospacedDigitSystemFont(ofSize: max(11, theme.fontSize * 0.7), weight: .regular),
         .foregroundColor: theme.secondaryColor]
    }

    private var timeText: NSString {
        "\(Self.clock(position)) / \(Self.clock(duration))" as NSString
    }

    /// The track runs from beside the button to before the time.
    private var track: CGRect {
        let left = button.maxX + 10
        let right = bounds.maxX - 12 - timeText.size(withAttributes: timeAttributes).width - 10
        return CGRect(x: left, y: bounds.midY + 5, width: max(0, right - left), height: 4)
    }

    private func fraction(at x: CGFloat) -> Double {
        let t = track
        guard t.width > 0 else { return 0 }
        return Double(min(1, max(0, (x - t.minX) / t.width)))
    }

    // MARK: drawing

    #if canImport(UIKit)
    override func draw(_ rect: CGRect) {
        guard let ctx = UIGraphicsGetCurrentContext() else { return }
        paint(in: ctx)
    }

    override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        setNeedsDisplay()
    }
    #elseif canImport(AppKit)
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        paint(in: ctx)
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }
    #endif

    /// The same chip the unplayed box was — fill, frame, tinted button — so
    /// activating it changes what is inside the box and not the box.
    private func paint(in ctx: CGContext) {
        let box = bounds.insetBy(dx: 0.5, dy: 0.5)
        let rounded = CGPath(roundedRect: box, cornerWidth: MediaMetrics.corner,
                             cornerHeight: MediaMetrics.corner, transform: nil)
        ctx.addPath(rounded)
        ctx.setFillColor(theme.codeBackground.cgColor)
        ctx.fillPath()
        ctx.addPath(rounded)
        ctx.setStrokeColor(theme.tableBorderColor.cgColor)
        ctx.setLineWidth(1)
        ctx.strokePath()

        // The button: the tint, so it is the obvious place to touch in either
        // appearance, with a white glyph.
        let b = button
        ctx.setFillColor(theme.handleColor.cgColor)
        ctx.fillEllipse(in: b)
        ctx.setFillColor(gray: 1, alpha: 1)
        if isPlaying {
            let w = b.width * 0.11, h = b.height * 0.38, gap = b.width * 0.1
            ctx.fill(CGRect(x: b.midX - gap / 2 - w, y: b.midY - h / 2, width: w, height: h))
            ctx.fill(CGRect(x: b.midX + gap / 2, y: b.midY - h / 2, width: w, height: h))
        } else {
            let s = b.width * 0.36
            let ox = b.midX + s * 0.12
            ctx.beginPath()
            ctx.move(to: CGPoint(x: ox - s * 0.5, y: b.midY - s * 0.6))
            ctx.addLine(to: CGPoint(x: ox - s * 0.5, y: b.midY + s * 0.6))
            ctx.addLine(to: CGPoint(x: ox + s * 0.6, y: b.midY))
            ctx.closePath()
            ctx.fillPath()
        }

        let t = track
        // The name, over the track, clipped to it.
        let nameAttrs: [NSAttributedString.Key: Any] = [
            .font: theme.proportionalFont(size: max(11, theme.fontSize * 0.75), bold: false, italic: false),
            .foregroundColor: theme.textColor,
        ]
        let label = name as NSString
        let nameHeight = label.size(withAttributes: nameAttrs).height
        ctx.saveGState()
        ctx.clip(to: CGRect(x: t.minX, y: bounds.minY, width: t.width, height: bounds.height))
        drawText(label, at: CGPoint(x: t.minX, y: t.minY - 5 - nameHeight), attributes: nameAttrs)
        ctx.restoreGState()

        // The track: dim to the end, the tint up to where playback is.
        let played = duration > 0 ? CGFloat(position / duration) : 0
        let radius = t.height / 2
        ctx.addPath(CGPath(roundedRect: t, cornerWidth: radius, cornerHeight: radius, transform: nil))
        ctx.setFillColor(theme.secondaryColor.withAlphaComponent(0.3).cgColor)
        ctx.fillPath()
        if played > 0 {
            let done = CGRect(x: t.minX, y: t.minY, width: max(t.height, t.width * played), height: t.height)
            ctx.addPath(CGPath(roundedRect: done, cornerWidth: radius, cornerHeight: radius, transform: nil))
            ctx.setFillColor(theme.handleColor.cgColor)
            ctx.fillPath()
        }
        let knob: CGFloat = 10
        ctx.setFillColor(theme.handleColor.cgColor)
        ctx.fillEllipse(in: CGRect(x: t.minX + t.width * played - knob / 2, y: t.midY - knob / 2,
                                   width: knob, height: knob))

        // The time, at the right, centred on the strip.
        let time = timeText
        let size = time.size(withAttributes: timeAttributes)
        drawText(time, at: CGPoint(x: bounds.maxX - 12 - size.width, y: bounds.midY - size.height / 2),
                 attributes: timeAttributes)
    }

    private func drawText(_ text: NSString, at point: CGPoint, attributes: [NSAttributedString.Key: Any]) {
        text.draw(at: point, withAttributes: attributes)
    }

    // MARK: input

    #if canImport(UIKit)
    @objc private func handleTap(_ gesture: UITapGestureRecognizer) {
        let point = gesture.location(in: self)
        if point.x < track.minX - 4 {
            toggle()
        } else {
            seek(to: fraction(at: point.x) * duration)
        }
    }

    @objc private func handlePan(_ gesture: UIPanGestureRecognizer) {
        let x = gesture.location(in: self).x
        switch gesture.state {
        case .began, .changed:
            scrubbing = fraction(at: x) * duration
            redraw()
        case .ended:
            scrubbing = nil
            seek(to: fraction(at: x) * duration)
        default:
            scrubbing = nil
            redraw()
        }
    }

    /// A drag on the strip scrubs only when it starts sideways; one that
    /// starts up or down is the page being scrolled, and is the scroll view's.
    override func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
        if let pan = gesture as? UIPanGestureRecognizer, pan.view === self {
            let v = pan.velocity(in: self)
            return abs(v.x) > abs(v.y)
        }
        return super.gestureRecognizerShouldBegin(gesture)
    }

    override var accessibilityLabel: String? {
        get { name }
        set {}
    }

    override func accessibilityActivate() -> Bool {
        toggle()
        return true
    }

    override func accessibilityIncrement() { seek(to: position + 15) }
    override func accessibilityDecrement() { seek(to: position - 15) }
    #elseif canImport(AppKit)
    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if point.x < track.minX - 4 {
            toggle()
        } else {
            scrubbing = fraction(at: point.x) * duration
            redraw()
        }
    }

    override func mouseDragged(with event: NSEvent) {
        guard scrubbing != nil else { return }
        scrubbing = fraction(at: convert(event.locationInWindow, from: nil).x) * duration
        redraw()
    }

    override func mouseUp(with event: NSEvent) {
        guard scrubbing != nil else { return }
        scrubbing = nil
        seek(to: fraction(at: convert(event.locationInWindow, from: nil).x) * duration)
    }

    override func accessibilityLabel() -> String? { name }
    override func accessibilityValue() -> Any? { "\(Self.clock(position)) of \(Self.clock(duration))" }
    override func accessibilityPerformPress() -> Bool {
        toggle()
        return true
    }
    #endif

    /// `m:ss`, or `h:mm:ss` past the hour.
    static func clock(_ seconds: Double) -> String {
        let total = Int(seconds.rounded(.down))
        let h = total / 3600, m = (total % 3600) / 60, s = total % 60
        return h > 0 ? String(format: "%d:%02d:%02d", h, m, s) : String(format: "%d:%02d", m, s)
    }
}
