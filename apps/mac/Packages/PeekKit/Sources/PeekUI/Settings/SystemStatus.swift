import AppKit
import CoreGraphics
import Foundation
import PeekCore
import ServiceManagement

// MARK: - Launch at login

/// The registration state of one `SMAppService` (the login item or the peekd agent).
public enum ServiceRegistrationState: Equatable, Sendable {
    case enabled
    case requiresApproval
    case notRegistered
    case notFound
    /// `PEEK_NO_SERVICES=1`: this run never touches SMAppService, so there is nothing to read.
    case disabledForThisRun
    case unknown(Int)

    init(_ status: SMAppService.Status) {
        switch status {
        case .enabled: self = .enabled
        case .requiresApproval: self = .requiresApproval
        case .notRegistered: self = .notRegistered
        case .notFound: self = .notFound
        @unknown default: self = .unknown(status.rawValue)
        }
    }

    public var isHealthy: Bool { self == .enabled }
}

/// Launch-at-login status shown in Settings › Startup (BLUEPRINT §1.8, §8.11).
public struct LoginItemStatus: Equatable, Sendable {
    /// `SMAppService.mainApp`: Peek.app opens at login.
    public var app: ServiceRegistrationState
    /// `SMAppService.agent("ai.tos.peek.daemon.plist")`: launchd keeps peekd running.
    public var helper: ServiceRegistrationState

    public init(app: ServiceRegistrationState, helper: ServiceRegistrationState) {
        self.app = app
        self.helper = helper
    }

    /// One sentence about the login item: what the state means and what to do.
    public var appSummary: String {
        switch app {
        case .enabled:
            "On. Peek opens when you log in."
        case .requiresApproval:
            "Waiting for your approval. Turn Peek on in System Settings › General › Login Items & Extensions."
        case .notRegistered:
            "Off. Peek registers itself when it launches from ~/Applications/Peek.app; isolated development runs "
                + "(PEEK_NO_SERVICES=1) and --simulate launches skip that."
        case .notFound:
            "macOS has no login item for this copy of Peek. This happens when Peek runs from a build folder "
                + "instead of ~/Applications/Peek.app."
        case .unknown(let raw):
            "macOS reports an unknown login-item status (\(raw)). Check System Settings › General › Login Items & Extensions."
        case .disabledForThisRun:
            "Not checked: this is an isolated run (PEEK_NO_SERVICES=1), which never registers or reads the login item."
        }
    }

    /// One sentence about the peekd agent.
    public var helperSummary: String {
        switch helper {
        case .enabled:
            "On. launchd keeps peekd running in the background and restarts it if it exits."
        case .requiresApproval:
            "Needs your approval in System Settings › General › Login Items & Extensions (listed under Peek). "
                + "Until then Peek starts peekd itself while the app is open, and Silicons' peeks wait while it is closed."
        case .notRegistered:
            "Not registered. Peek starts peekd itself while the app is open; relaunch Peek from "
                + "~/Applications/Peek.app to register the background helper."
        case .notFound:
            "This build has no Contents/Library/LaunchAgents/ai.tos.peek.daemon.plist, so macOS cannot run peekd "
                + "in the background. Reinstall Peek (the CLI does this on the next peek command)."
        case .unknown(let raw):
            "macOS reports an unknown status (\(raw)) for the peekd background helper."
        case .disabledForThisRun:
            "Not managed: this is an isolated run (PEEK_NO_SERVICES=1). Peek neither registers nor starts peekd; it "
                + "connects to the peekd on PEEK_DAEMON_SOCKET."
        }
    }

    /// Whether the approval hint (and the Login Items button) should be prominent.
    public var needsApproval: Bool { app == .requiresApproval || helper == .requiresApproval }
}

/// Reads launch-at-login state. Reading never registers or changes anything.
@MainActor
public protocol LoginItemStatusProviding: AnyObject {
    func currentStatus() -> LoginItemStatus
    /// Opens System Settings › General › Login Items & Extensions.
    func openLoginItemsSettings()
}

/// The live provider over `SMAppService`. Registration itself belongs to the App target's
/// `ServiceRegistration`; this type only reads `status`.
@MainActor
public final class SMAppServiceStatusProvider: LoginItemStatusProviding {
    public static let agentPlistName = "ai.tos.peek.daemon.plist"

    private let environment: PeekRuntimeEnvironment

    public init(environment: PeekRuntimeEnvironment = .current) {
        self.environment = environment
    }

    public func currentStatus() -> LoginItemStatus {
        // Isolated runs never touch SMAppService, not even to read.
        if environment.noServices { return LoginItemStatus(app: .disabledForThisRun, helper: .disabledForThisRun) }
        return LoginItemStatus(
            app: ServiceRegistrationState(SMAppService.mainApp.status),
            helper: ServiceRegistrationState(SMAppService.agent(plistName: Self.agentPlistName).status))
    }

    public func openLoginItemsSettings() {
        SMAppService.openSystemSettingsLoginItems()
    }
}

// MARK: - Screen Recording (backdrop "screen")

/// Screen Recording access for the opt-in `screen` backdrop source (BLUEPRINT §8.8).
@MainActor
public protocol ScreenCapturePermissionProviding: AnyObject {
    /// `CGPreflightScreenCaptureAccess()`: true when Peek may capture the screen. Never prompts.
    func isGranted() -> Bool
    /// `CGRequestScreenCaptureAccess()`: prompts once; after a denial macOS only offers System Settings.
    func request() -> Bool
    /// Opens System Settings › Privacy & Security › Screen & System Audio Recording.
    func openSystemSettings()
}

@MainActor
public final class ScreenCapturePermission: ScreenCapturePermissionProviding {
    static let settingsURL = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")

    public init() {}

    public func isGranted() -> Bool { CGPreflightScreenCaptureAccess() }

    public func request() -> Bool { CGRequestScreenCaptureAccess() }

    public func openSystemSettings() {
        if let url = Self.settingsURL { NSWorkspace.shared.open(url) }
    }
}

// MARK: - Versions

/// Version facts about the running Peek.app for Diagnostics.
public struct SettingsAppInfo: Equatable, Sendable {
    public var version: String?
    public var build: String?
    public var bundleIdentifier: String?

    public init(version: String?, build: String?, bundleIdentifier: String?) {
        self.version = version
        self.build = build
        self.bundleIdentifier = bundleIdentifier
    }

    public init(bundle: Bundle) {
        let info = bundle.infoDictionary ?? [:]
        self.init(
            version: info["CFBundleShortVersionString"] as? String, build: info["CFBundleVersion"] as? String,
            bundleIdentifier: bundle.bundleIdentifier)
    }

    /// `0.1.0 (1000)`, or a sentence when the process is not a Peek.app bundle (tests, `swift run`).
    public var versionText: String {
        switch (version, build) {
        case let (version?, build?): "\(version) (\(build))"
        case let (version?, nil): version
        case let (nil, build?): "build \(build)"
        case (nil, nil): "unknown (not running from a Peek.app bundle)"
        }
    }

    public var isDevelopmentBuild: Bool { bundleIdentifier == "ai.tos.peek.dev" }
}

extension MicPermission {
    /// Diagnostics wording for the microphone permission.
    var diagnosticsText: String {
        switch self {
        case .granted: "allowed"
        case .denied: "not allowed; turn Peek on in System Settings › Privacy & Security › Microphone to answer by voice"
        case .undetermined: "not asked yet; Peek asks the first time you answer by voice"
        case .restricted: "restricted by a device-management profile or parental controls; answer by typing"
        }
    }
}
