import Foundation
import Observation
import PeekCore

/// The model behind Settings (BLUEPRINT §8.11): typed accessors over ``PeekSettings`` whose writes
/// go through ``PeekControlling/setSetting(_:_:)``, which persists settings.json and mirrors the
/// change to peekd with `settings.changed`.
///
/// Every write is validated here first with the same rules settings.json uses
/// (``PeekSettings/apply(_:_:)``), so an invalid value never reaches the file or peekd and the
/// Carbon sees the precise reason next to the control.
@MainActor
@Observable
public final class SettingsModel {
    public let controls: any PeekControlling
    @ObservationIgnored public let loginItems: any LoginItemStatusProviding
    @ObservationIgnored public let screenCapture: any ScreenCapturePermissionProviding

    /// The last rejected change, as a full sentence; cleared by the next accepted change.
    public private(set) var lastError: String?
    public private(set) var loginStatus: LoginItemStatus?
    public private(set) var screenCaptureGranted: Bool?
    /// The draft of a typed STT language tag (committed with ``commitCustomSTTLanguage()``).
    public var customSTTLanguage = ""
    public private(set) var customSTTProblem: String?
    /// "Other…" was chosen: keep showing the text field until a listed choice is picked.
    public private(set) var wantsCustomSTT = false

    public init(controls: any PeekControlling, loginItems: (any LoginItemStatusProviding)? = nil,
                screenCapture: (any ScreenCapturePermissionProviding)? = nil) {
        self.controls = controls
        self.loginItems = loginItems ?? SMAppServiceStatusProvider()
        self.screenCapture = screenCapture ?? ScreenCapturePermission()
        let stt = controls.settings.sttLanguage
        if stt != STTLanguageChoices.auto, !STTLanguageChoices.isCommon(stt) {
            customSTTLanguage = stt
            wantsCustomSTT = true
        }
    }

    public var settings: PeekSettings { controls.settings }

    // MARK: Writing

    /// Validates `value` for `key`, then applies it through the coordinator. Returns whether it was accepted.
    @discardableResult
    public func apply(_ key: PeekSettings.Key, _ value: JSONValue) -> Bool {
        var candidate = controls.settings
        do throws(SettingsError) {
            try candidate.apply(key, value)
        } catch {
            lastError = error.description
            return false
        }
        lastError = nil
        if candidate.value(for: key) == controls.settings.value(for: key) { return true }
        controls.setSetting(key, candidate.value(for: key))
        return true
    }

    public func setMode(_ mode: DisplayMode) { apply(.mode, .string(mode.rawValue)) }
    public func setHotkeyModifier(_ modifier: HotkeyModifier) { apply(.hotkeyModifier, .string(modifier.rawValue)) }
    public func setDisplay(_ display: DisplayTarget) { apply(.display, .string(display.rawValue)) }
    public func setTelemetry(_ enabled: Bool) { apply(.telemetry, .bool(enabled)) }

    public func setBackdrop(_ backdrop: BackdropSourceSetting) {
        apply(.backdrop, .string(backdrop.rawValue))
        refreshScreenCapture()
        // Choosing Screen is the Carbon's explicit request: ask macOS right away (it prompts only the first time; after
        // a refusal the hint and the "Allow Screen Recording…" button point to System Settings).
        if backdrop == .screen, screenCaptureGranted == false { requestScreenCapture() }
    }

    // MARK: Voices

    /// The voice peekd uses for `language` when neither `--voice` nor the Silicon's config sets one.
    public func voice(for language: String) -> String {
        settings.voiceDefaults[language] ?? VoiceCatalog.defaultVoice(for: language) ?? ""
    }

    public func isVoiceCustomized(for language: String) -> Bool {
        voice(for: language) != VoiceCatalog.defaultVoice(for: language)
    }

    public var hasCustomVoices: Bool { VoiceCatalog.languages.contains { isVoiceCustomized(for: $0) } }

    public func setVoice(_ voice: String, for language: String) {
        var voices = settings.voiceDefaults
        voices[language] = voice
        apply(.voiceDefaults, .object(voices.mapValues(JSONValue.string)))
    }

    public func resetVoice(for language: String) {
        guard let original = VoiceCatalog.defaultVoice(for: language) else { return }
        setVoice(original, for: language)
    }

    public func resetAllVoices() { apply(.voiceDefaults, .null) }

    // MARK: Speech-to-text language

    public enum STTSelection: Hashable, Sendable {
        case automatic
        case tag(String)
        /// The Carbon types a tag.
        case custom
    }

    public var sttSelection: STTSelection {
        if wantsCustomSTT { return .custom }
        let current = settings.sttLanguage
        if current == STTLanguageChoices.auto { return .automatic }
        return STTLanguageChoices.isCommon(current) ? .tag(current) : .custom
    }

    public func selectSTT(_ selection: STTSelection) {
        switch selection {
        case .automatic:
            wantsCustomSTT = false
            customSTTProblem = nil
            apply(.sttLanguage, .string(STTLanguageChoices.auto))
        case .tag(let tag):
            wantsCustomSTT = false
            customSTTProblem = nil
            apply(.sttLanguage, .string(tag))
        case .custom:
            wantsCustomSTT = true
            if customSTTLanguage.isEmpty, settings.sttLanguage != STTLanguageChoices.auto {
                customSTTLanguage = settings.sttLanguage
            }
            if STTLanguageChoices.problem(with: customSTTLanguage) == nil { commitCustomSTTLanguage() }
        }
    }

    /// Applies ``customSTTLanguage`` when it is a valid tag; otherwise records why not.
    public func commitCustomSTTLanguage() {
        let tag = customSTTLanguage.trimmingCharacters(in: .whitespaces)
        if let problem = STTLanguageChoices.problem(with: tag) {
            customSTTProblem = problem
            return
        }
        customSTTProblem = nil
        customSTTLanguage = tag
        apply(.sttLanguage, .string(tag))
    }

    // MARK: System state

    public func refreshSystemStatus() {
        loginStatus = loginItems.currentStatus()
        refreshScreenCapture()
    }

    public func refreshScreenCapture() {
        screenCaptureGranted = screenCapture.isGranted()
    }

    /// Asks macOS for Screen Recording access (only from an explicit click).
    public func requestScreenCapture() {
        screenCaptureGranted = screenCapture.request() || screenCapture.isGranted()
    }

    /// The hint under the Backdrop picker; nil when nothing needs saying.
    public var backdropHint: String? {
        switch settings.backdrop {
        case .wallpaper:
            return "Peek samples your desktop picture under each bubble. No permission is needed."
        case .screen:
            guard screenCaptureGranted == false else {
                return "Peek samples the screen under each visible bubble twice a second, excluding its own windows. "
                    + "macOS may ask you to confirm Screen Recording again from time to time."
            }
            return "Screen Recording is not allowed for Peek, so bubbles use the desktop picture instead. "
                + "Allow Peek in System Settings › Privacy & Security › Screen & System Audio Recording."
        }
    }

}

extension DaemonLinkState {
    /// The link state as a sentence for the menu bar and Diagnostics.
    public var statusSentence: String {
        switch self {
        case .idle:
            "Not connected to peekd yet."
        case .connecting(let attempt):
            attempt > 1 ? "Connecting to peekd (attempt \(attempt))…" : "Connecting to peekd…"
        case .connected(let hello):
            "Connected to peekd \(hello.peekdVersion ?? "(version not reported)"), protocol \(hello.protocolVersion)."
        case .waiting(let retryIn, let reason):
            "\(reason.hasSuffix(".") ? reason : reason + ".") Retrying in \(Self.seconds(retryIn))."
        case .stopped:
            "Disconnected from peekd (Peek is quitting)."
        }
    }

    /// Short form for the menu bar header.
    public var shortStatus: String {
        switch self {
        case .idle: "starting"
        case .connecting: "connecting…"
        case .connected(let hello): hello.peekdVersion.map { "peekd \($0)" } ?? "connected"
        case .waiting: "peekd unavailable"
        case .stopped: "stopped"
        }
    }

    static func seconds(_ duration: Duration) -> String {
        let components = duration.components
        let value = Double(components.seconds) + Double(components.attoseconds) / 1e18
        return value < 10 ? String(format: "%.1f s", value) : String(format: "%.0f s", value)
    }
}
