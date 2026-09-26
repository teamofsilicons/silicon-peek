import AppKit
import Foundation
import PeekCore

/// The system appearance (`input.appearance`, B8) and its changes. Injected for tests.
@MainActor
public protocol AppearanceProviding: AnyObject {
    var current: Appearance { get }
    var onChange: (@MainActor (Appearance) -> Void)? { get set }
}

/// `NSApp.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua])`, observed with KVO (BLUEPRINT §8.3).
@MainActor
public final class SystemAppearance: AppearanceProviding {
    public var onChange: (@MainActor (Appearance) -> Void)?
    public private(set) var current: Appearance

    private var observation: NSKeyValueObservation?
    private weak var application: NSApplication?

    /// - Parameter application: the app to observe; defaults to `NSApp` (nil in unit tests, which then read `.light`
    ///   until an application exists).
    public init(application: NSApplication? = NSApp) {
        current = Self.appearance(of: application?.effectiveAppearance)
        attach(application)
    }

    /// Maps an `NSAppearance` onto light or dark.
    public static func appearance(of appearance: NSAppearance?) -> Appearance {
        appearance?.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? .dark : .light
    }

    private func attach(_ application: NSApplication?) {
        guard let application else { return }
        self.application = application
        let changed = MainThread.callback { [weak self] in self?.refresh() }
        observation = application.observe(\.effectiveAppearance, options: [.new]) { _, _ in changed() }
    }

    private func refresh() {
        let next = Self.appearance(of: application?.effectiveAppearance)
        guard next != current else { return }
        current = next
        onChange?(next)
    }
}
