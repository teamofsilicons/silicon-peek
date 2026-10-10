import Foundation
import Observation
import PeekCore

// Tokens stay in peekd; native settings only choose a saved account and enable deliveries.
struct PermissionActor: Codable, Sendable, Equatable {
    let type: String
    let publicID: String
    enum CodingKeys: String, CodingKey { case type; case publicID = "public_id" }
}

struct PermissionContext: Codable, Sendable, Equatable, Identifiable {
    let homeID: String
    let contextID: String
    let actor: PermissionActor
    let accountID: String
    let apiURL: String
    let label: String
    var id: String { homeID + "\u{1f}" + contextID }
    enum CodingKeys: String, CodingKey {
        case homeID = "home_id", contextID = "context_id", actor, accountID = "account_id"
        case apiURL = "api_url", label
    }
}

struct PermissionContextsRequest: UIRequest {
    static let op = "permissions.contexts"
    struct Reply: Decodable, Sendable { let contexts: [PermissionContext] }
}

enum PermissionAction: String, Encodable, Sendable { case status, enroll }

struct TingPermissionRequest: UIRequest {
    static let op = "permissions.ting"
    let homeID: String
    let contextID: String
    let action: PermissionAction
    enum CodingKeys: String, CodingKey { case homeID = "home_id", contextID = "context_id", action }
    struct Reply: Decodable, Sendable {
        let contextID: String
        let enrolled: Bool
        enum CodingKeys: String, CodingKey { case contextID = "context_id", enrolled }
    }
}

@MainActor @Observable
final class PermissionsModel {
    private(set) var contexts: [PermissionContext] = []
    private(set) var selectedID: String?
    private(set) var enrolled = false
    private(set) var busy = false
    private(set) var loadingContexts = false
    private(set) var error: String?
    private(set) var notice: String?
    @ObservationIgnored private let link: any DaemonLinking
    @ObservationIgnored private var epoch = UUID()

    init(link: any DaemonLinking) { self.link = link }
    var selected: PermissionContext? { contexts.first { $0.id == selectedID } }

    func select(_ id: String?) {
        guard id != selectedID else { return }
        suspend()
        selectedID = contexts.first { $0.id == id }?.id
        enrolled = false
        error = nil
        notice = nil
    }

    func suspend() {
        epoch = UUID()
        loadingContexts = false
        busy = false
    }

    func reloadContexts() async {
        let ticket = UUID()
        epoch = ticket
        loadingContexts = true
        error = nil
        do throws(DaemonLinkError) {
            let reply = try await link.send(PermissionContextsRequest())
            guard ticket == epoch, !Task.isCancelled else { return }
            guard Set(reply.contexts.map(\.id)).count == reply.contexts.count,
                  reply.contexts.allSatisfy({ !$0.homeID.isEmpty && !$0.contextID.isEmpty && UUID(uuidString: $0.accountID) != nil }) else {
                throw DaemonLinkError.invalidReply(op: PermissionContextsRequest.op, reason: "duplicate or missing account")
            }
            contexts = reply.contexts
            selectedID = contexts.first { $0.id == selectedID }?.id ?? contexts.first?.id
            loadingContexts = false
            await perform(.status)
        } catch {
            guard ticket == epoch, !Task.isCancelled else { return }
            loadingContexts = false
            self.error = error.description
        }
    }

    func perform(_ action: PermissionAction) async {
        guard let context = selected, !busy else { return }
        let ticket = epoch
        busy = true
        error = nil
        notice = nil
        defer { if ticket == epoch { busy = false } }
        do throws(DaemonLinkError) {
            let reply = try await link.send(TingPermissionRequest(homeID: context.homeID, contextID: context.contextID, action: action))
            guard ticket == epoch, selected == context, !Task.isCancelled else { return }
            guard reply.contextID == context.contextID, action != .enroll || reply.enrolled else {
                throw DaemonLinkError.invalidReply(op: TingPermissionRequest.op, reason: "delivery account did not match")
            }
            enrolled = reply.enrolled
            if action == .enroll { notice = "Deliveries enabled. Queued answers for this account can now retry." }
        } catch {
            guard ticket == epoch, !Task.isCancelled else { return }
            self.error = error.description
        }
    }
}
