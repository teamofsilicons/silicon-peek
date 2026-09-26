import AppKit
import Carbon.HIToolbox
import OSLog
import PeekCore

/// Which slots get a hotkey: every occupied slot whose `slots.state` entry asks for one
/// (BLUEPRINT §8.5: "Hotkeys are registered only for occupied slots").
public enum HotKeyPlan {
    public static func slots(for table: [SlotState]) -> Set<SlotIndex> {
        Set(table.filter { $0.hotkey && $0.context != .simulation }.map(\.index))
    }

    /// "⌘3", "⌃⌥5": the label for a slot's shortcut.
    public static func label(_ slot: SlotIndex, _ modifier: HotkeyModifier) -> String {
        "\(modifier.symbols)\(slot.rawValue)"
    }

    /// A precise message for a failed `RegisterEventHotKey`.
    public static func failureMessage(_ slot: SlotIndex, _ modifier: HotkeyModifier, status: OSStatus) -> String {
        let shortcut = label(slot, modifier)
        switch Int(status) {
        case eventHotKeyExistsErr:
            return "\(shortcut) for position \(slot.rawValue) is already taken by another app; "
                + "choose another hotkey modifier in Peek Settings › Keyboard"
        case eventHotKeyInvalidErr:
            return "macOS refused \(shortcut) as a global shortcut (error \(status)); "
                + "choose another hotkey modifier in Peek Settings › Keyboard"
        default:
            return "registering \(shortcut) for position \(slot.rawValue) failed with Carbon error \(status); "
                + "choose another hotkey modifier in Peek Settings › Keyboard"
        }
    }
}

/// Global per-slot shortcuts with Carbon `RegisterEventHotKey`, which needs no Accessibility or
/// Input Monitoring permission (notes/macos §3). Bare `\` and Esc are never registered globally:
/// they are handled by the summoned panel while it is key.
@MainActor
public final class HotKeyCenter {
    /// 'PEEK'
    nonisolated static let signature: OSType = 0x5045_454B

    public var onPress: (@MainActor (SlotIndex) -> Void)?
    public private(set) var registered: Set<SlotIndex> = []
    public private(set) var modifier: HotkeyModifier = PeekSettings.defaults.hotkeyModifier
    public private(set) var problems: [String] = []

    private var refs: [SlotIndex: EventHotKeyRef] = [:]
    private var handler: EventHandlerRef?
    private let logger = PeekLogger(category: "hotkeys")

    public init() {}

    /// Registers exactly `slots` with `modifier` (re-registering when the modifier changed).
    /// Returns the problems (already-taken shortcuts and similar), also kept in ``problems``.
    @discardableResult
    public func update(slots: Set<SlotIndex>, modifier: HotkeyModifier) -> [String] {
        if !slots.isEmpty { installHandlerIfNeeded() }
        if modifier != self.modifier {
            unregister(Set(refs.keys))
            self.modifier = modifier
        }
        unregister(Set(refs.keys).subtracting(slots))
        var problems: [String] = []
        for slot in slots.sorted() where refs[slot] == nil {
            var ref: EventHotKeyRef?
            let id = EventHotKeyID(signature: Self.signature, id: UInt32(slot.rawValue))
            let status = RegisterEventHotKey(slot.hotKeyCode, modifier.carbonMask, id, GetApplicationEventTarget(), 0, &ref)
            if status == noErr, let ref {
                refs[slot] = ref
            } else {
                let message = HotKeyPlan.failureMessage(slot, modifier, status: status)
                logger.error("\(message)")
                problems.append(message)
            }
        }
        registered = Set(refs.keys)
        self.problems = problems
        return problems
    }

    public func unregisterAll() {
        unregister(Set(refs.keys))
        registered = []
        if let handler {
            RemoveEventHandler(handler)
            self.handler = nil
        }
    }

    private func unregister(_ slots: Set<SlotIndex>) {
        for slot in slots {
            if let ref = refs.removeValue(forKey: slot) { UnregisterEventHotKey(ref) }
        }
        registered = Set(refs.keys)
    }

    private func installHandlerIfNeeded() {
        guard handler == nil else { return }
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let userData = Unmanaged.passUnretained(self).toOpaque()
        let status = InstallEventHandler(GetApplicationEventTarget(), hotKeyHandler, 1, &spec, userData, &handler)
        if status != noErr {
            let message = "installing the global hotkey handler failed with Carbon error \(status); the per-position shortcuts (\(modifier.symbols)1…8) will not work"
            logger.error("\(message)")
            problems.append(message)
        }
    }

    /// Returns false for hotkeys this center did not register (another center in the same app,
    /// e.g. Simulation's presenter, may own them), so Carbon passes the event on.
    fileprivate func pressed(_ id: UInt32) -> Bool {
        guard let slot = SlotIndex(rawValue: Int(id)), registered.contains(slot) else { return false }
        onPress?(slot)
        return true
    }
}

/// The Carbon callback (a C function pointer: no captures; `self` travels in `userData`).
/// Carbon delivers application-target events on the main thread.
private func hotKeyHandler(_ call: EventHandlerCallRef?, _ event: EventRef?, _ userData: UnsafeMutableRawPointer?) -> OSStatus {
    guard let event, let userData else { return OSStatus(eventNotHandledErr) }
    var id = EventHotKeyID()
    let status = GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil,
                                   MemoryLayout<EventHotKeyID>.size, nil, &id)
    guard status == noErr, id.signature == HotKeyCenter.signature else { return OSStatus(eventNotHandledErr) }
    let hotKeyID = id.id
    let center = Unmanaged<HotKeyCenter>.fromOpaque(userData).takeUnretainedValue()
    let handled = MainActor.assumeIsolated { center.pressed(hotKeyID) }
    return handled ? noErr : OSStatus(eventNotHandledErr)
}

/// Esc for an open tap-to-expand popup (ui-feedback.md #5). The popup's panel is not key (the Carbon is reading, not
/// typing), so the key never reaches it; while a popup is open, bare Esc is registered as a Carbon hot key (no
/// Accessibility permission needed) and released the moment the popup closes. Nothing else ever holds Esc globally.
@MainActor
final class EscapeHotKey {
    /// 'PEEC'
    nonisolated static let signature: OSType = 0x5045_4543
    nonisolated static let escapeKeyCode: UInt16 = 53

    var onPress: (@MainActor () -> Void)?
    private(set) var isActive = false
    private var ref: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private let id: UInt32
    private let logger = PeekLogger(category: "hotkeys")

    private static var nextID: UInt32 = 1

    init() {
        id = Self.nextID
        Self.nextID &+= 1
    }

    func setActive(_ active: Bool) {
        guard active != isActive else { return }
        isActive = active
        if active {
            var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
            let userData = Unmanaged.passUnretained(self).toOpaque()
            if InstallEventHandler(GetApplicationEventTarget(), escapeHotKeyHandler, 1, &spec, userData, &handler) != noErr {
                handler = nil
            }
            let hotKeyID = EventHotKeyID(signature: Self.signature, id: id)
            let status = RegisterEventHotKey(UInt32(Self.escapeKeyCode), 0, hotKeyID, GetApplicationEventTarget(), 0, &ref)
            if status != noErr {
                ref = nil
                logger.notice("Esc for the expanded text is unavailable (Carbon error \(status)); click to close it")
            }
        } else {
            if let ref { UnregisterEventHotKey(ref) }
            ref = nil
            if let handler { RemoveEventHandler(handler) }
            handler = nil
        }
    }

    fileprivate func pressed(_ hotKeyID: EventHotKeyID) -> Bool {
        guard isActive, hotKeyID.signature == Self.signature, hotKeyID.id == id else { return false }
        onPress?()
        return true
    }
}

private func escapeHotKeyHandler(_ call: EventHandlerCallRef?, _ event: EventRef?, _ userData: UnsafeMutableRawPointer?)
    -> OSStatus
{
    guard let event, let userData else { return OSStatus(eventNotHandledErr) }
    var id = EventHotKeyID()
    let status = GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil,
                                   MemoryLayout<EventHotKeyID>.size, nil, &id)
    guard status == noErr, id.signature == EscapeHotKey.signature else { return OSStatus(eventNotHandledErr) }
    let owner = Unmanaged<EscapeHotKey>.fromOpaque(userData).takeUnretainedValue()
    let handled = MainActor.assumeIsolated { owner.pressed(id) }
    return handled ? noErr : OSStatus(eventNotHandledErr)
}
