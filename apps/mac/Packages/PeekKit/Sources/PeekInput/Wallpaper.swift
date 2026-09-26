import AppKit
import Foundation
import ImageIO
import OSLog
import PeekCore

/// A display, as the backdrop sampler needs it.
public struct ScreenInfo: Sendable, Hashable {
    /// `CGDirectDisplayID` (`NSScreenNumber`).
    public var id: UInt32
    /// The full frame in AppKit global coordinates (y up).
    public var frame: CGRect
    public var backingScale: CGFloat

    public init(id: UInt32, frame: CGRect, backingScale: CGFloat = 2) {
        self.id = id
        self.frame = frame
        self.backingScale = backingScale
    }

    /// The screen a rectangle belongs to: the one containing its centre, else the one it overlaps most.
    public static func screen(for rect: CGRect, in screens: [ScreenInfo]) -> ScreenInfo? {
        let center = CGPoint(x: rect.midX, y: rect.midY)
        if let containing = screens.first(where: { $0.frame.contains(center) }) { return containing }
        return screens.max { lhs, rhs in
            area(lhs.frame.intersection(rect)) < area(rhs.frame.intersection(rect))
        }.flatMap { area($0.frame.intersection(rect)) > 0 ? $0 : nil }
    }

    private static func area(_ rect: CGRect) -> CGFloat { rect.isNull ? 0 : rect.width * rect.height }
}

/// How the desktop picture is laid on a screen (`NSWorkspace.desktopImageOptions`: scaling, clipping, fill colour).
public enum WallpaperLayout {
    /// Where an image of `imageSize` points lands on a screen of `screenSize` points, in screen-local coordinates
    /// (y-down from the screen's top-left). The macOS modes map as: Fill = proportionally up/down + clipping,
    /// Fit = proportionally up/down without clipping, Stretch = axes independently, Centre = none.
    public static func imageFrame(imageSize: CGSize, screenSize: CGSize, scaling: NSImageScaling,
                                  allowClipping: Bool) -> CGRect {
        guard imageSize.width > 0, imageSize.height > 0, screenSize.width > 0, screenSize.height > 0 else {
            return CGRect(origin: .zero, size: screenSize)
        }
        let fit = min(screenSize.width / imageSize.width, screenSize.height / imageSize.height)
        let fill = max(screenSize.width / imageSize.width, screenSize.height / imageSize.height)
        let scale: CGFloat
        switch scaling {
        case .scaleAxesIndependently:
            return CGRect(origin: .zero, size: screenSize)
        case .scaleNone:
            scale = 1
        case .scaleProportionallyDown:
            scale = min(1, allowClipping ? fill : fit)
        case .scaleProportionallyUpOrDown:
            scale = allowClipping ? fill : fit
        @unknown default:
            scale = allowClipping ? fill : fit
        }
        let size = CGSize(width: imageSize.width * scale, height: imageSize.height * scale)
        return CGRect(x: (screenSize.width - size.width) / 2, y: (screenSize.height - size.height) / 2,
                      width: size.width, height: size.height)
    }
}

/// Identifies what a screen shows as its desktop picture; a change means the thumbnail must be decoded again.
public struct WallpaperKey: Sendable, Hashable {
    public var url: URL
    public var fileSize: Int?
    public var modified: Date?
    public var scaling: NSImageScaling
    public var allowClipping: Bool
    public var fillColor: SRGBColor?
    public var screen: ScreenInfo

    public init(url: URL, fileSize: Int? = nil, modified: Date? = nil,
                scaling: NSImageScaling = .scaleProportionallyUpOrDown, allowClipping: Bool = true,
                fillColor: SRGBColor? = nil, screen: ScreenInfo) {
        self.url = url
        self.fileSize = fileSize
        self.modified = modified
        self.scaling = scaling
        self.allowClipping = allowClipping
        self.fillColor = fillColor
        self.screen = screen
    }
}

/// A decoded desktop picture thumbnail (≤ 256 px) and where it lies on its screen.
public struct WallpaperSnapshot: Sendable, Equatable {
    public var bitmap: RGBABitmap
    /// Where the image lands, in screen-local points (y-down from the screen's top-left).
    public var imageFrame: CGRect
    /// Shown where the image does not cover the screen (Fit and Centre modes).
    public var fillColor: SRGBColor
    public var screen: ScreenInfo

    public init(bitmap: RGBABitmap, imageFrame: CGRect, fillColor: SRGBColor, screen: ScreenInfo) {
        self.bitmap = bitmap
        self.imageFrame = imageFrame
        self.fillColor = fillColor
        self.screen = screen
    }

    /// The average colour of the desktop picture under `rectOnScreen` (AppKit global points): image pixels where
    /// the image covers the rect, the fill colour elsewhere, weighted by area. Nil when the rect misses the screen.
    public func averageColor(under rectOnScreen: CGRect) -> SRGBColor? {
        let local = CGRect(x: rectOnScreen.minX - screen.frame.minX, y: screen.frame.maxY - rectOnScreen.maxY,
                           width: rectOnScreen.width, height: rectOnScreen.height)
        let onScreen = local.intersection(CGRect(origin: .zero, size: screen.frame.size))
        guard !onScreen.isNull, onScreen.width > 0, onScreen.height > 0 else { return nil }
        let total = Double(onScreen.width * onScreen.height)
        let covered = onScreen.intersection(imageFrame)
        var imageColor: SRGBColor?
        var imageShare = 0.0
        if !covered.isNull, covered.width > 0, covered.height > 0, imageFrame.width > 0, imageFrame.height > 0 {
            let sx = CGFloat(bitmap.width) / imageFrame.width
            let sy = CGFloat(bitmap.height) / imageFrame.height
            let pixels = CGRect(x: (covered.minX - imageFrame.minX) * sx, y: (covered.minY - imageFrame.minY) * sy,
                                width: covered.width * sx, height: covered.height * sy)
            if let average = bitmap.averageColor(in: pixels) {
                // Transparent parts of the picture show the fill colour.
                imageColor = fillColor.mixed(with: average.color, amount: average.opacity)
                imageShare = Double(covered.width * covered.height) / total
            }
        }
        guard let imageColor else { return fillColor }
        return fillColor.mixed(with: imageColor, amount: imageShare)
    }
}

/// Reads the desktop picture of each screen. Injected into ``BackdropSampler`` for tests.
@MainActor
public protocol WallpaperProviding: AnyObject {
    var screens: [ScreenInfo] { get }
    /// What the screen shows now. Cheap: no decoding. Nil when there is no readable desktop picture.
    func currentKey(for screen: ScreenInfo) -> WallpaperKey?
    /// Decodes the thumbnail for `key` (off the main thread).
    func snapshot(for key: WallpaperKey) async -> WallpaperSnapshot?
    /// The space or the screen configuration changed: desktop pictures may differ now.
    var onEnvironmentChange: (@MainActor () -> Void)? { get set }
}

/// The live desktop picture: `NSWorkspace.desktopImageURL(for:)` + `desktopImageOptions(for:)`, re-checked on
/// `activeSpaceDidChangeNotification` and `didChangeScreenParametersNotification` (BLUEPRINT §8.3). There is no
/// public "wallpaper changed" notification; ``BackdropSampler`` also re-checks every 60 s.
@MainActor
public final class SystemWallpaper: WallpaperProviding {
    public var onEnvironmentChange: (@MainActor () -> Void)?
    private var observers: [(NotificationCenter, any NSObjectProtocol)] = []

    public init() {
        let changed = MainThread.callback { [weak self] in self?.onEnvironmentChange?() }
        let workspaceCenter = NSWorkspace.shared.notificationCenter
        observers.append((workspaceCenter, workspaceCenter.addObserver(
            forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main) { _ in changed() }))
        observers.append((NotificationCenter.default, NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { _ in changed() }))
    }

    isolated deinit {
        for (center, observer) in observers { center.removeObserver(observer) }
    }

    public var screens: [ScreenInfo] {
        NSScreen.screens.compactMap { screen in
            guard let number = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber else {
                return nil
            }
            return ScreenInfo(id: number.uint32Value, frame: screen.frame, backingScale: screen.backingScaleFactor)
        }
    }

    public func currentKey(for screen: ScreenInfo) -> WallpaperKey? {
        guard let nsScreen = NSScreen.screens.first(where: {
            ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == screen.id
        }), let url = NSWorkspace.shared.desktopImageURL(for: nsScreen) else { return nil }
        let options = NSWorkspace.shared.desktopImageOptions(for: nsScreen) ?? [:]
        let scaling = (options[.imageScaling] as? NSNumber).flatMap { NSImageScaling(rawValue: $0.uintValue) }
            ?? .scaleProportionallyUpOrDown
        let clipping = (options[.allowClipping] as? NSNumber)?.boolValue ?? true
        let fill = (options[.fillColor] as? NSColor)?.usingColorSpace(.sRGB).map {
            SRGBColor(red: Double($0.redComponent), green: Double($0.greenComponent), blue: Double($0.blueComponent))
        }
        let attributes = try? FileManager.default.attributesOfItem(atPath: url.path)
        return WallpaperKey(url: url, fileSize: (attributes?[.size] as? NSNumber)?.intValue,
                            modified: attributes?[.modificationDate] as? Date, scaling: scaling,
                            allowClipping: clipping, fillColor: fill, screen: screen)
    }

    public func snapshot(for key: WallpaperKey) async -> WallpaperSnapshot? {
        await Task.detached(priority: .utility) { WallpaperDecoder.snapshot(for: key) }.value
    }
}

/// Decodes a desktop picture into a ≤ 256 px bitmap (off the main thread).
public enum WallpaperDecoder {
    public static let maxPixelSize = 256
    private static let logger = PeekLogger(category: "backdrop")

    public static func snapshot(for key: WallpaperKey) -> WallpaperSnapshot? {
        guard let source = CGImageSourceCreateWithURL(key.url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary),
            CGImageSourceGetCount(source) > 0
        else {
            logger.notice("cannot read the desktop picture \(key.url.path); using the appearance")
            return nil
        }
        let index = CGImageSourceGetPrimaryImageIndex(source)
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixelSize,
            kCGImageSourceCreateThumbnailWithTransform: true,
        ]
        guard let thumbnail = CGImageSourceCreateThumbnailAtIndex(source, index, options as CFDictionary),
            let bitmap = RGBABitmap(image: thumbnail)
        else {
            logger.notice("cannot decode the desktop picture \(key.url.path); using the appearance")
            return nil
        }
        // The picture's size in points: its pixel size (orientation applied) at the screen's backing scale.
        var pixelSize = CGSize(width: thumbnail.width, height: thumbnail.height)
        if let properties = CGImageSourceCopyPropertiesAtIndex(source, index, nil) as? [CFString: Any],
            let width = (properties[kCGImagePropertyPixelWidth] as? NSNumber)?.doubleValue,
            let height = (properties[kCGImagePropertyPixelHeight] as? NSNumber)?.doubleValue, width > 0, height > 0
        {
            let orientation = (properties[kCGImagePropertyOrientation] as? NSNumber)?.intValue ?? 1
            pixelSize = orientation >= 5 ? CGSize(width: height, height: width) : CGSize(width: width, height: height)
        }
        let scale = max(key.screen.backingScale, 1)
        let imageSize = CGSize(width: pixelSize.width / scale, height: pixelSize.height / scale)
        let frame = WallpaperLayout.imageFrame(imageSize: imageSize, screenSize: key.screen.frame.size,
                                               scaling: key.scaling, allowClipping: key.allowClipping)
        return WallpaperSnapshot(bitmap: bitmap, imageFrame: frame, fillColor: key.fillColor ?? .black,
                                 screen: key.screen)
    }
}
