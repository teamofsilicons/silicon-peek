import AppKit
import PeekCore

/// Which screen bubbles go to (BLUEPRINT §8.5 "Display"): the menu-bar screen (`NSScreen.screens[0]`)
/// by default, or the screen under the pointer at send time when Settings says "Follow pointer".
public enum ScreenPolicy {
    /// The visible frame used when no screen is available (headless runs, tests).
    public static let fallbackVisibleFrame = CGRect(x: 0, y: 0, width: 1512, height: 944)

    /// Index of the screen whose frame contains `point`, else the menu-bar screen (0). Pure.
    public static func screenIndex(for target: DisplayTarget, pointer: CGPoint, frames: [CGRect]) -> Int? {
        guard !frames.isEmpty else { return nil }
        switch target {
        case .main:
            return 0
        case .pointer:
            // NSMouseInRect semantics: the top and right edges belong to the neighbouring screen.
            return frames.firstIndex { frame in
                pointer.x >= frame.minX && pointer.x < frame.maxX && pointer.y > frame.minY && pointer.y <= frame.maxY
            } ?? 0
        }
    }

    /// `visibleFrame` of the chosen screen (menu bar and Dock excluded).
    @MainActor
    public static func visibleFrame(for target: DisplayTarget) -> CGRect {
        let screens = NSScreen.screens
        guard let index = screenIndex(for: target, pointer: NSEvent.mouseLocation, frames: screens.map(\.frame)) else {
            return fallbackVisibleFrame
        }
        return screens[index].visibleFrame
    }
}
