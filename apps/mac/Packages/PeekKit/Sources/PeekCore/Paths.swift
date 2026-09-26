import Foundation

/// Per-OS-user locations (§1.7). `home` is the real account home from
/// `getpwuid`, never `$HOME` or `$SILICON_HOME`: Peek.app is one per OS user,
/// and launchd starts it without the shell's environment.
///
/// Isolated runs (docs/development.md "Isolated run mode") move the two
/// Peek-owned directories with `PEEK_SUPPORT_DIR` (replaces
/// `~/Library/Application Support/Peek`) and `PEEK_CACHES_DIR` (replaces
/// `~/Library/Caches/Peek`). peekd honours the same `PEEK_SUPPORT_DIR`, so an
/// isolated app and daemon share their settings, logs and image cache. An empty
/// value means "not set", as in peekd.
///
/// Tests construct `PeekPaths(home:)` with a temporary directory; nothing in
/// PeekKit writes to ``current`` on its own.
public struct PeekPaths: Sendable, Equatable {
    public static let supportDirectoryEnvironment = "PEEK_SUPPORT_DIR"
    public static let cachesDirectoryEnvironment = "PEEK_CACHES_DIR"

    public var home: URL
    /// `PEEK_SUPPORT_DIR`: replaces `~/Library/Application Support/Peek` when set.
    public var supportOverride: URL?
    /// `PEEK_CACHES_DIR`: replaces `~/Library/Caches/Peek` when set.
    public var cachesOverride: URL?

    public init(home: URL, supportDirectory: URL? = nil, cachesDirectory: URL? = nil) {
        self.home = home
        supportOverride = supportDirectory
        cachesOverride = cachesDirectory
    }

    /// The logged-in user's real home directory.
    public static func realUserHome() -> URL {
        if let entry = getpwuid(getuid()), let dir = entry.pointee.pw_dir {
            return URL(fileURLWithPath: String(cString: dir), isDirectory: true)
        }
        return FileManager.default.homeDirectoryForCurrentUser
    }

    /// The real home with the `PEEK_SUPPORT_DIR` / `PEEK_CACHES_DIR` overrides from `environment`.
    /// Relative override paths resolve against the current directory.
    public static func fromEnvironment(_ environment: [String: String] = ProcessInfo.processInfo.environment,
                                       home: URL = realUserHome()) -> PeekPaths {
        func directory(_ name: String) -> URL? {
            guard let raw = environment[name], !raw.isEmpty else { return nil }
            return URL(fileURLWithPath: raw, isDirectory: true).standardizedFileURL
        }
        return PeekPaths(home: home, supportDirectory: directory(supportDirectoryEnvironment),
                         cachesDirectory: directory(cachesDirectoryEnvironment))
    }

    /// This process's paths: the real home plus the environment overrides.
    public static var current: PeekPaths { fromEnvironment() }

    /// Whether either Peek directory is redirected (an isolated run).
    public var isIsolated: Bool { supportOverride != nil || cachesOverride != nil }

    /// `~/Library/Application Support/Peek/` (0700), or `PEEK_SUPPORT_DIR`.
    public var supportDirectory: URL {
        supportOverride ?? home.appendingPathComponent("Library/Application Support/Peek", isDirectory: true)
    }

    public var settingsFile: URL { supportDirectory.appendingPathComponent("settings.json") }
    public var uiLog: URL { supportDirectory.appendingPathComponent("ui.log") }
    public var peekdLog: URL { supportDirectory.appendingPathComponent("peekd.log") }
    public var drawingsDirectory: URL { supportDirectory.appendingPathComponent("drawings", isDirectory: true) }
    public var imageCacheDirectory: URL { supportDirectory.appendingPathComponent("cache/images", isDirectory: true) }
    public var recordingsDirectory: URL { supportDirectory.appendingPathComponent("recordings", isDirectory: true) }
    public var installStatusFile: URL { supportDirectory.appendingPathComponent("install-status.txt") }
    public var updateAppliedFile: URL { supportDirectory.appendingPathComponent("update-applied.json") }

    /// `~/Library/Caches/Peek/` (or `PEEK_CACHES_DIR`): Simulation's generated samples.
    public var cachesDirectory: URL {
        cachesOverride ?? home.appendingPathComponent("Library/Caches/Peek", isDirectory: true)
    }

    /// `~/Applications/Peek.app`, where the CLI installs and peekd updates the app (D12).
    public var installedApp: URL { home.appendingPathComponent("Applications/Peek.app", isDirectory: true) }
}

/// The process-wide switches for local and isolated runs (docs/development.md "Isolated run mode").
///
/// * `PEEK_NO_SERVICES=1`: Peek.app never touches SMAppService (no login item, no launchd agent, not even a
///   status read), never spawns peekd, and `--uninstall` leaves services alone. It connects to whatever
///   peekd listens on `PEEK_DAEMON_SOCKET`.
/// * `PEEK_SKIP_SERVICE_REGISTRATION=1`: the older, narrower switch (registration only); kept as an alias.
/// * `PEEK_API_URL`: the peek-server origin override (loopback `http` allowed). Peek.app itself makes no
///   HTTP calls; it passes the variable to a peekd it spawns and shows it in Settings › Diagnostics.
public struct PeekRuntimeEnvironment: Sendable, Equatable {
    public static let noServicesVariable = "PEEK_NO_SERVICES"
    public static let skipServiceRegistrationVariable = "PEEK_SKIP_SERVICE_REGISTRATION"
    public static let apiURLVariable = "PEEK_API_URL"
    public static let daemonSocketVariable = "PEEK_DAEMON_SOCKET"

    /// Services (SMAppService, the peekd fallback spawn) are off for this process.
    public var noServices: Bool
    /// `PEEK_API_URL`, when set.
    public var apiURL: String?
    /// `PEEK_DAEMON_SOCKET`, when set.
    public var daemonSocket: String?

    public init(noServices: Bool = false, apiURL: String? = nil, daemonSocket: String? = nil) {
        self.noServices = noServices
        self.apiURL = apiURL
        self.daemonSocket = daemonSocket
    }

    public static func fromEnvironment(_ environment: [String: String] = ProcessInfo.processInfo.environment)
        -> PeekRuntimeEnvironment
    {
        func flag(_ name: String) -> Bool {
            guard let raw = environment[name]?.lowercased() else { return false }
            return ["1", "true", "yes", "on"].contains(raw)
        }
        func value(_ name: String) -> String? {
            guard let raw = environment[name], !raw.isEmpty else { return nil }
            return raw
        }
        return PeekRuntimeEnvironment(
            noServices: flag(noServicesVariable) || flag(skipServiceRegistrationVariable),
            apiURL: value(apiURLVariable), daemonSocket: value(daemonSocketVariable))
    }

    public static var current: PeekRuntimeEnvironment { fromEnvironment() }
}
