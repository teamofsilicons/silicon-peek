import AppKit
import CoreGraphics
import Foundation
import ScreenCaptureKit

/// What a capture failed on, as a full sentence.
public struct ScreenCaptureError: Error, Sendable, Equatable, CustomStringConvertible {
    public var description: String
    public init(_ description: String) { self.description = description }
}

/// Samples the live screen under a rectangle, excluding Peek's own windows (the opt-in `screen` backdrop, §8.8).
/// Injected into ``BackdropSampler`` for tests.
@MainActor
public protocol ScreenCapturing: AnyObject {
    /// Screen Recording is granted. Checking never prompts.
    var hasAccess: Bool { get }
    /// The average colour under `rectOnScreen` (AppKit global points), from a 16 × 16 capture.
    func averageColor(under rectOnScreen: CGRect) async throws(ScreenCaptureError) -> SRGBColor
    /// The display configuration changed: forget cached displays.
    func invalidate()
}

/// Converts between AppKit global coordinates (origin at the bottom-left of the primary display, y up) and
/// Quartz display coordinates (origin at the top-left of the main display, y down), which `CGDisplayBounds`
/// and ScreenCaptureKit's `sourceRect` use.
public enum ScreenCoordinates {
    /// `rect` (AppKit global) in the Quartz global space; `primaryHeight` is the primary display's height.
    public static func quartzRect(fromAppKit rect: CGRect, primaryHeight: CGFloat) -> CGRect {
        CGRect(x: rect.minX, y: primaryHeight - rect.maxY, width: rect.width, height: rect.height)
    }

    /// `rect` (AppKit global) relative to a display's top-left, for `SCStreamConfiguration.sourceRect`.
    public static func displayLocalRect(fromAppKit rect: CGRect, displayBounds: CGRect,
                                        primaryHeight: CGFloat) -> CGRect {
        let quartz = quartzRect(fromAppKit: rect, primaryHeight: primaryHeight)
        return CGRect(x: quartz.minX - displayBounds.minX, y: quartz.minY - displayBounds.minY,
                      width: quartz.width, height: quartz.height)
            .intersection(CGRect(origin: .zero, size: displayBounds.size))
    }
}

/// ScreenCaptureKit at 16 × 16 (BLUEPRINT §8.3 "Backdrop (opt-in)"): `SCContentFilter(display:excludingApplications:
/// [Peek]:exceptingWindows:[])`, `sourceRect` in display points, `SCScreenshotManager.captureImage`. Never asks for
/// permission itself: Settings calls ``requestAccess()`` when the Carbon opts in.
@MainActor
public final class ScreenCaptureKitSampler: ScreenCapturing {
    public static let captureSize = 16
    /// Shareable content (displays, our own app) is cached this long.
    public static let contentLifetime: TimeInterval = 10

    private var content: SCShareableContent?
    private var contentFetchedAt: Date?

    public init() {}

    public var hasAccess: Bool { CGPreflightScreenCaptureAccess() }

    /// Shows the system Screen Recording prompt (once per app identity). Returns whether access is granted now;
    /// macOS may require relaunching Peek.app after the Carbon allows it.
    @discardableResult
    public static func requestAccess() -> Bool { CGRequestScreenCaptureAccess() }

    public func averageColor(under rectOnScreen: CGRect) async throws(ScreenCaptureError) -> SRGBColor {
        guard hasAccess else {
            throw ScreenCaptureError(
                "Peek.app has no Screen Recording access; allow it in System Settings › Privacy & Security › "
                    + "Screen & System Audio Recording, or switch the backdrop to Wallpaper")
        }
        let content = try await shareableContent()
        let primaryHeight = CGDisplayBounds(CGMainDisplayID()).height
        let quartz = ScreenCoordinates.quartzRect(fromAppKit: rectOnScreen, primaryHeight: primaryHeight)
        let center = CGPoint(x: quartz.midX, y: quartz.midY)
        guard let display = content.displays.first(where: { CGDisplayBounds($0.displayID).contains(center) }) else {
            throw ScreenCaptureError("no display contains the bubble at \(rectOnScreen); the screen layout changed")
        }
        let bounds = CGDisplayBounds(display.displayID)
        let local = ScreenCoordinates.displayLocalRect(fromAppKit: rectOnScreen, displayBounds: bounds,
                                                       primaryHeight: primaryHeight)
        guard !local.isNull, local.width >= 1, local.height >= 1 else {
            throw ScreenCaptureError("the bubble at \(rectOnScreen) is off its display")
        }
        let pid = ProcessInfo.processInfo.processIdentifier
        let own = content.applications.filter { $0.processID == pid }
        let filter = SCContentFilter(display: display, excludingApplications: own, exceptingWindows: [])
        let configuration = SCStreamConfiguration()
        configuration.sourceRect = local
        configuration.width = Self.captureSize
        configuration.height = Self.captureSize
        configuration.showsCursor = false
        configuration.colorSpaceName = CGColorSpace.sRGB
        let image: CGImage
        do {
            image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: configuration)
        } catch {
            self.content = nil
            throw ScreenCaptureError("ScreenCaptureKit could not capture the backdrop: \(error.localizedDescription)")
        }
        guard let bitmap = RGBABitmap(image: image),
            let average = bitmap.averageColor(in: CGRect(x: 0, y: 0, width: bitmap.width, height: bitmap.height))
        else {
            throw ScreenCaptureError("the backdrop capture came back empty")
        }
        return average.color
    }

    /// Forgets cached displays (call when the screen configuration changes).
    public func invalidate() {
        content = nil
        contentFetchedAt = nil
    }

    private func shareableContent() async throws(ScreenCaptureError) -> SCShareableContent {
        if let content, let contentFetchedAt, Date().timeIntervalSince(contentFetchedAt) < Self.contentLifetime {
            return content
        }
        do {
            let fresh = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
            content = fresh
            contentFetchedAt = Date()
            return fresh
        } catch {
            throw ScreenCaptureError("ScreenCaptureKit could not list the displays: \(error.localizedDescription)")
        }
    }
}
