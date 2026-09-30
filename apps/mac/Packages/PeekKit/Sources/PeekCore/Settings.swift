import Foundation

/// `~/Library/Application Support/Peek/settings.json` (§1.7): Peek.app and peekd settings.
///
/// Decoding is lenient per key: a wrong or unknown value falls back to its default
/// with a warning, and keys this build does not know are kept in ``extra`` and
/// written back unchanged, so a newer peekd's keys survive an older UI.
public struct PeekSettings: Sendable, Equatable {
    public static let schema: Int64 = 1

    /// The keys, as sent in `settings.changed.key`.
    public enum Key: String, CaseIterable, Sendable {
        case mode
        case hotkeyModifier = "hotkey_modifier"
        case display
        case backdrop
        case telemetry
        case showTestPeeks = "show_test_peeks"
        case voiceDefaults = "voice_defaults"
        case sttLanguage = "stt_language"
        case cliWatchdog = "updates.cli_watchdog"
    }

    public var mode: DisplayMode = .normal
    public var hotkeyModifier: HotkeyModifier = .ctrlCmd
    public var display: DisplayTarget = .main
    public var backdrop: BackdropSourceSetting = .wallpaper
    public var telemetry = true
    public var showTestPeeks = true
    /// Language (BCP 47 primary subtag) → ElevenLabs voice.
    public var voiceDefaults: [String: String] = DefaultVoices.byLanguage
    /// `auto` or a BCP 47 tag.
    public var sttLanguage = "auto"
    public var cliWatchdog = true
    /// Top-level keys this build does not understand, preserved verbatim.
    public var extra: [String: JSONValue] = [:]

    public init() {}

    public static let defaults = PeekSettings()

    // MARK: Key access

    public func value(for key: Key) -> JSONValue {
        switch key {
        case .mode: .string(mode.rawValue)
        case .hotkeyModifier: .string(hotkeyModifier.rawValue)
        case .display: .string(display.rawValue)
        case .backdrop: .string(backdrop.rawValue)
        case .telemetry: .bool(telemetry)
        case .showTestPeeks: .bool(showTestPeeks)
        case .voiceDefaults: .object(voiceDefaults.mapValues(JSONValue.string))
        case .sttLanguage: .string(sttLanguage)
        case .cliWatchdog: .bool(cliWatchdog)
        }
    }

    /// Sets one key from its JSON value (the `settings.changed` shape). `null` resets it to the default.
    public mutating func apply(_ key: Key, _ value: JSONValue) throws(SettingsError) {
        if value.isNull {
            let defaults = PeekSettings.defaults
            try apply(key, defaults.value(for: key))
            return
        }
        switch key {
        case .mode: mode = try Self.enumValue(key, value)
        case .hotkeyModifier: hotkeyModifier = try Self.enumValue(key, value)
        case .display: display = try Self.enumValue(key, value)
        case .backdrop: backdrop = try Self.enumValue(key, value)
        case .telemetry: telemetry = try Self.boolValue(key, value)
        case .showTestPeeks: showTestPeeks = try Self.boolValue(key, value)
        case .cliWatchdog: cliWatchdog = try Self.boolValue(key, value)
        case .sttLanguage:
            guard let text = value.stringValue, text == "auto" || Self.isLanguageTag(text) else {
                throw SettingsError(key: key.rawValue, message: "must be \"auto\" or a BCP 47 language tag such as \"en\" or \"pt-BR\"; got \(value.jsonString)")
            }
            sttLanguage = text
        case .voiceDefaults:
            guard let object = value.objectValue else {
                throw SettingsError(key: key.rawValue, message: "must be an object of language → ElevenLabs voice; got \(value.jsonString)")
            }
            var voices: [String: String] = [:]
            for (language, voice) in object {
                guard (2...3).contains(language.utf8.count), Self.isLanguageTag(language) else {
                    throw SettingsError(
                        key: key.rawValue,
                        message: "language \"\(language)\" must be a lowercase 2–3 letter primary language code")
                }
                guard let name = voice.stringValue, DefaultVoices.isValidVoice(name) else {
                    throw SettingsError(
                        key: key.rawValue,
                        message: "voice for \"\(language)\" must be an ElevenLabs voice ID (1–128 ASCII letters, digits, _ or -); got \(voice.jsonString)")
                }
                voices[language] = name
            }
            voiceDefaults = DefaultVoices.byLanguage.merging(voices) { _, chosen in chosen }
        }
    }

    // MARK: JSON

    /// The settings.json document (keys sorted; `updates` nested as in §1.7).
    public var jsonValue: JSONValue {
        var object = extra
        object["schema"] = .int(Self.schema)
        for key in Key.allCases where key != .cliWatchdog {
            object[key.rawValue] = value(for: key)
        }
        var updates = extra["updates"]?.objectValue ?? [:]
        updates["cli_watchdog"] = .bool(cliWatchdog)
        object["updates"] = .object(updates)
        return .object(object)
    }

    /// Decodes settings.json leniently. Returns the settings and one warning per value that was ignored.
    public static func decode(_ data: Data) -> (settings: PeekSettings, warnings: [String]) {
        var settings = PeekSettings()
        var warnings: [String] = []
        let root: JSONValue
        do {
            root = try StrictJSON.parse(data)
        } catch {
            return (settings, ["settings.json is not valid JSON (\(error)); using defaults"])
        }
        guard case .object(var object) = root else {
            return (settings, ["settings.json must hold a JSON object; using defaults"])
        }
        if let schema = object.removeValue(forKey: "schema"), schema.intValue.map({ $0 > Int(Self.schema) }) == true {
            warnings.append("settings.json schema \(schema.jsonString) is newer than this build (\(Self.schema)); unknown keys are kept")
        }
        if let updates = object.removeValue(forKey: "updates") {
            if let watchdog = updates["cli_watchdog"] {
                do throws(SettingsError) {
                    try settings.apply(.cliWatchdog, watchdog)
                } catch {
                    warnings.append(error.description)
                }
            }
            var rest = updates.objectValue ?? [:]
            rest.removeValue(forKey: "cli_watchdog")
            if !rest.isEmpty { settings.extra["updates"] = .object(rest) }
        }
        for key in Key.allCases where key != .cliWatchdog {
            guard let value = object.removeValue(forKey: key.rawValue) else { continue }
            do throws(SettingsError) {
                try settings.apply(key, value)
            } catch {
                warnings.append(error.description)
            }
        }
        settings.extra.merge(object) { current, _ in current }
        return (settings, warnings)
    }

    /// Reads settings.json. A missing file is not a warning; defaults apply.
    public static func load(from url: URL) -> (settings: PeekSettings, warnings: [String]) {
        do {
            let data = try Data(contentsOf: url)
            return decode(data)
        } catch let error as CocoaError where error.code == .fileReadNoSuchFile {
            return (PeekSettings(), [])
        } catch {
            return (PeekSettings(), ["cannot read \(url.path) (\(error.localizedDescription)); using defaults"])
        }
    }

    /// Writes settings.json atomically with mode 0600 inside a 0700 directory.
    public func write(to url: URL) throws {
        var bytes: [UInt8] = []
        jsonValue.write(into: &bytes)
        bytes.append(0x0A)
        try AtomicFile.write(Data(bytes), to: url)
    }

    // MARK: Helpers

    private static func enumValue<T: RawRepresentable & CaseIterable>(_ key: Key, _ value: JSONValue) throws(SettingsError) -> T
    where T.RawValue == String {
        guard let raw = value.stringValue, let parsed = T(rawValue: raw) else {
            let allowed = T.allCases.map { "\"\($0.rawValue)\"" }.joined(separator: ", ")
            throw SettingsError(key: key.rawValue, message: "must be one of \(allowed); got \(value.jsonString)")
        }
        return parsed
    }

    private static func boolValue(_ key: Key, _ value: JSONValue) throws(SettingsError) -> Bool {
        guard let flag = value.boolValue else {
            throw SettingsError(key: key.rawValue, message: "must be true or false; got \(value.jsonString)")
        }
        return flag
    }

    /// A pragmatic BCP 47 check: a 2–3 letter language, then 2–8 character alphanumeric subtags.
    public static func isLanguageTag(_ tag: String) -> Bool {
        let parts = tag.split(separator: "-", omittingEmptySubsequences: false)
        guard let language = parts.first, (2...3).contains(language.count),
            language.allSatisfy({ $0.isASCII && $0.isLowercase && $0.isLetter })
        else { return false }
        return parts.dropFirst().allSatisfy { part in
            (2...8).contains(part.count) && part.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber) }
        }
    }
}

public struct SettingsError: Error, Equatable, Sendable, CustomStringConvertible {
    public var key: String
    public var message: String

    public init(key: String, message: String) {
        self.key = key
        self.message = message
    }

    public var description: String { "settings key \"\(key)\" \(message)" }
}

/// Crash-safe file replacement (§1.7): temp file created with O_EXCL and 0600,
/// written, fsynced, renamed over the target, then the directory is fsynced.
public enum AtomicFile {
    public struct WriteError: Error, CustomStringConvertible, Sendable {
        public let description: String
    }

    public static func write(_ data: Data, to url: URL, mode: mode_t = 0o600, directoryMode: mode_t = 0o700) throws {
        let directory = url.deletingLastPathComponent()
        do {
            try FileManager.default.createDirectory(
                at: directory, withIntermediateDirectories: true,
                attributes: [.posixPermissions: NSNumber(value: directoryMode)])
        } catch {
            throw WriteError(description: "cannot create \(directory.path): \(error.localizedDescription)")
        }
        let tempPath = directory.appendingPathComponent(".\(url.lastPathComponent).\(getpid()).\(UInt32.random(in: 0...UInt32.max)).tmp").path
        let fd = open(tempPath, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, mode)
        guard fd >= 0 else {
            throw WriteError(description: "cannot create \(tempPath): \(String(cString: strerror(errno)))")
        }
        var ok = false
        defer {
            if !ok { unlink(tempPath) }
        }
        let written = data.withUnsafeBytes { buffer -> Int in
            var offset = 0
            while offset < buffer.count {
                let n = Darwin.write(fd, buffer.baseAddress!.advanced(by: offset), buffer.count - offset)
                if n < 0 {
                    if errno == EINTR { continue }
                    return -1
                }
                offset += n
            }
            return offset
        }
        let writeErrno = errno
        guard written == data.count, fsync(fd) == 0 else {
            close(fd)
            throw WriteError(description: "cannot write \(tempPath): \(String(cString: strerror(writeErrno)))")
        }
        close(fd)
        guard rename(tempPath, url.path) == 0 else {
            throw WriteError(description: "cannot replace \(url.path): \(String(cString: strerror(errno)))")
        }
        ok = true
        let dirFD = open(directory.path, O_RDONLY | O_CLOEXEC)
        if dirFD >= 0 {
            _ = fsync(dirFD)
            close(dirFD)
        }
    }
}
