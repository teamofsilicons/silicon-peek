#if DEBUG
import AppKit
import PeekCore

/// Debug builds only (`--simulate-backdrop <image>`): a click-through window that fills the menu-bar screen with
/// an image, above other apps' windows and below the slot panels. Screenshots of a simulated bubble then show a
/// known backdrop (and nothing private from other apps), and the glass refracts that image.
@MainActor
enum SimulationBackdropWindow {
    private static var window: NSWindow?
    private static let logger = PeekLogger(category: "simulation")

    static func show(imagePath: String) {
        guard let screen = NSScreen.screens.first else { return }
        guard let image = NSImage(contentsOfFile: imagePath) else {
            logger.error("--simulate-backdrop: cannot read an image at \(imagePath)")
            FileHandle.standardError.write(Data("peek simulation: cannot read the backdrop image \(imagePath)\n".utf8))
            return
        }
        window?.orderOut(nil)
        let frame = screen.frame
        let window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isOpaque = true
        window.backgroundColor = .black
        window.hasShadow = false
        window.ignoresMouseEvents = true
        window.isReleasedWhenClosed = false
        window.animationBehavior = .none
        window.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .fullScreenAuxiliary]
        // Just below the slot panels (.floating), above ordinary windows.
        window.level = NSWindow.Level(rawValue: NSWindow.Level.floating.rawValue - 1)
        let view = NSView(frame: NSRect(origin: .zero, size: frame.size))
        view.wantsLayer = true
        view.layer?.contentsGravity = .resizeAspectFill
        view.layer?.masksToBounds = true
        view.layer?.contents = image.cgImage(forProposedRect: nil, context: nil, hints: nil)
        window.contentView = view
        window.setFrame(frame, display: true)
        window.orderFrontRegardless()
        self.window = window
    }
}
#endif
