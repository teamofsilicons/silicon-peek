import Foundation

/// What a key press means in a summoned (key) slot panel (BLUEPRINT §8.5 "Hotkeys"):
///   * `\` → voice mode (matched by `charactersIgnoringModifiers`, not the key code, so every
///     layout works); in voice mode `\` or Return stops and sends;
///   * a printable character → typing mode, seeded with that character;
///   * Esc → cancel and slide back (never while an input method is composing);
///   * Return → submit (the typing field submits on its own);
///   * arrows move the highlighted option or nudge the slider; Space toggles a multiple-choice option.
public enum KeyCommand: Sendable, Equatable {
    case voice
    case type(String)
    case cancel
    case submit
    case move(Int)
    case toggle
    /// Not ours: let the panel (or the focused text field) handle it.
    case pass
}

public enum KeyContext: Sendable, Equatable {
    /// Summoned, no input yet.
    case idle
    /// The typing field has focus. `composing` while an input method has marked text.
    case typing(composing: Bool)
    /// Recording.
    case listening
    /// Waiting for peekd (transcribing or submitting).
    case busy
}

public enum KeyClassifier {
    // Carbon virtual key codes (Events.h).
    public static let escape: UInt16 = 0x35
    public static let returnKey: UInt16 = 0x24
    public static let keypadEnter: UInt16 = 0x4C
    public static let space: UInt16 = 0x31
    public static let leftArrow: UInt16 = 0x7B
    public static let rightArrow: UInt16 = 0x7C
    public static let downArrow: UInt16 = 0x7D
    public static let upArrow: UInt16 = 0x7E

    /// - Parameters:
    ///   - commandOrControl: ⌘ or ⌃ is held (those keys are shortcuts, never text).
    public static func classify(characters: String?, charactersIgnoringModifiers: String?, keyCode: UInt16,
                                commandOrControl: Bool, context: KeyContext) -> KeyCommand {
        if keyCode == escape {
            if case .typing(let composing) = context, composing { return .pass }
            return .cancel
        }
        switch context {
        case .typing, .busy:
            return .pass
        case .listening:
            if keyCode == returnKey || keyCode == keypadEnter { return .submit }
            if !commandOrControl, charactersIgnoringModifiers == "\\" { return .submit }
            return .pass
        case .idle:
            if commandOrControl { return .pass }
            if keyCode == returnKey || keyCode == keypadEnter { return .submit }
            switch keyCode {
            case leftArrow, upArrow: return .move(-1)
            case rightArrow, downArrow: return .move(1)
            case space: return .toggle
            default: break
            }
            if charactersIgnoringModifiers == "\\" { return .voice }
            if let characters, isPrintable(characters) { return .type(characters) }
            return .pass
        }
    }

    /// Text a key press would type: not empty, no control characters, not a function key
    /// (AppKit maps arrows and F-keys into U+F700–U+F8FF).
    public static func isPrintable(_ text: String) -> Bool {
        guard !text.isEmpty else { return false }
        return text.unicodeScalars.allSatisfy { scalar in
            !(scalar.value < 0x20 || scalar.value == 0x7F || (0xF700...0xF8FF).contains(scalar.value))
        }
    }
}
