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
/// Input Monitoring permission (notes/macos §3). Bare `\` is never registered globally: it is handled by the summoned
/// panel while it is key. Bare Esc is held only briefly, by ``CarbonEscapeKey`` for the Esc router (a new peek's 3 s
/// grace, hover, an Esc window the Carbon opened, an open popup).
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

/// Where the Esc router (``SlotManager``) gets bare Esc from. The app uses ``CarbonEscapeKey``; tests and GUI-less runs
/// use ``InertEscapeKey`` or a fake. Nothing here ever uses Input Monitoring or Accessibility.
@MainActor
public protocol EscapeKeyProviding: AnyObject {
    /// A press while held. Return true when it was used.
    var onPress: (@MainActor () -> Bool)? { get set }
    /// With several clients in one app (the app's slots and Simulation's), the one whose newest Esc target is the most
    /// recent gets the press first.
    var priority: (@MainActor () -> Double)? { get set }
    /// Whether this client asked for Esc.
    var isHeld: Bool { get }
    /// Asks for (true) or gives back (false) bare Esc. Returns a sentence when it could not be registered (another app
    /// holds it); the client then works without Esc until it asks again.
    @discardableResult
    func setHeld(_ held: Bool) -> String?
}

/// Never registers anything (tests, headless runs).
@MainActor
public final class InertEscapeKey: EscapeKeyProviding {
    public var onPress: (@MainActor () -> Bool)?
    public var priority: (@MainActor () -> Double)?
    public private(set) var isHeld = false

    public init() {}

    @discardableResult
    public func setHeld(_ held: Bool) -> String? {
        isHeld = held
        return nil
    }
}

/// Bare Esc as a Carbon hot key (`RegisterEventHotKey(kVK_Escape, 0)`, signature 'PEES'): no Accessibility or Input
/// Monitoring permission (notes/macos §3). One registration serves every client in the app; it exists only while at
/// least one client holds it, so Esc belongs to the frontmost app the rest of the time (agreed 0.1.2 #12, #13).
@MainActor
public final class CarbonEscapeKey: EscapeKeyProviding {
    public var onPress: (@MainActor () -> Bool)?
    public var priority: (@MainActor () -> Double)?
    public private(set) var isHeld = false

    public init() {
        CarbonEscapeCenter.shared.add(self)
    }

    isolated deinit {
        CarbonEscapeCenter.shared.remove(self)
    }

    @discardableResult
    public func setHeld(_ held: Bool) -> String? {
        guard held != isHeld else { return nil }
        isHeld = held
        return CarbonEscapeCenter.shared.update()
    }
}

/// The app-wide Carbon registration behind every ``CarbonEscapeKey``.
@MainActor
final class CarbonEscapeCenter {
    static let shared = CarbonEscapeCenter()
    /// 'PEES'
    nonisolated static let signature: OSType = 0x5045_4553
    nonisolated static let escapeKeyCode: UInt32 = 53

    private final class WeakClient {
        weak var client: CarbonEscapeKey?
        init(_ client: CarbonEscapeKey) { self.client = client }
    }

    private var clients: [ObjectIdentifier: WeakClient] = [:]
    private var ref: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private let logger = PeekLogger(category: "hotkeys")

    /// Whether bare Esc is registered right now (diagnostics).
    var isRegistered: Bool { ref != nil }

    func add(_ client: CarbonEscapeKey) { clients[ObjectIdentifier(client)] = WeakClient(client) }

    func remove(_ client: CarbonEscapeKey) {
        clients.removeValue(forKey: ObjectIdentifier(client))
        _ = update()
    }

    /// Registers bare Esc while any client holds it, releases it otherwise. Returns a problem when registering failed.
    func update() -> String? {
        clients = clients.filter { $0.value.client != nil }
        let wanted = clients.values.contains { $0.client?.isHeld == true }
        if wanted, ref == nil {
            if handler == nil {
                var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
                let userData = Unmanaged.passUnretained(self).toOpaque()
                if InstallEventHandler(GetApplicationEventTarget(), escapeHotKeyHandler, 1, &spec, userData, &handler) != noErr {
                    handler = nil
                }
            }
            var newRef: EventHotKeyRef?
            let status = RegisterEventHotKey(Self.escapeKeyCode, 0, EventHotKeyID(signature: Self.signature, id: 1),
                                             GetApplicationEventTarget(), 0, &newRef)
            guard status == noErr, let newRef else {
                logger.notice("registering bare Esc failed with Carbon error \(status); peeks work without Esc meanwhile")
                return Self.problem(status: status)
            }
            ref = newRef
            logger.debug("bare Esc registered for peeks")
        } else if !wanted, let current = ref {
            UnregisterEventHotKey(current)
            ref = nil
            logger.debug("bare Esc given back")
            if let handler { RemoveEventHandler(handler) }
            handler = nil
        }
        return nil
    }

    static func problem(status: OSStatus) -> String {
        Int(status) == eventHotKeyExistsErr
            ? "Esc for peeks is taken by another app; use the down-arrow"
            : "Esc for peeks is unavailable (Carbon error \(status)); use the down-arrow"
    }

    /// Hands the press to the holding client with the most recent target; false when none used it.
    fileprivate func pressed() -> Bool {
        let holders = clients.values.compactMap(\.client).filter(\.isHeld)
            .sorted { ($0.priority?() ?? 0) > ($1.priority?() ?? 0) }
        for client in holders where client.onPress?() == true { return true }
        return false
    }
}

private func escapeHotKeyHandler(_ call: EventHandlerCallRef?, _ event: EventRef?, _ userData: UnsafeMutableRawPointer?)
    -> OSStatus
{
    guard let event, let userData else { return OSStatus(eventNotHandledErr) }
    var id = EventHotKeyID()
    let status = GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil,
                                   MemoryLayout<EventHotKeyID>.size, nil, &id)
    guard status == noErr, id.signature == CarbonEscapeCenter.signature else { return OSStatus(eventNotHandledErr) }
    let center = Unmanaged<CarbonEscapeCenter>.fromOpaque(userData).takeUnretainedValue()
    let handled = MainActor.assumeIsolated { center.pressed() }
    return handled ? noErr : OSStatus(eventNotHandledErr)
}
