import Foundation
import Observation
import PeekCore

// Local IPC contracts belong to this pane. Tokens and retry receipts stay in peekd.
struct PermissionActor: Codable, Sendable, Equatable {
    let type: String
    let publicID: String
    enum CodingKeys: String, CodingKey { case type; case publicID = "public_id" }
}

struct PermissionContext: Codable, Sendable, Equatable, Identifiable {
    let homeID: String
    let contextID: String
    let actor: PermissionActor
    let orgID: String
    let apiURL: String
    let context: String
    let label: String
    var id: String { homeID + "\u{1f}" + contextID }
    var environmentLabel: String { context == "production" ? "Production" : "Testing · " + context }
    enum CodingKeys: String, CodingKey {
        case homeID = "home_id", contextID = "context_id", actor, orgID = "org_id"
        case apiURL = "api_url", context, label
    }
}

struct PermissionContextsRequest: UIRequest {
    static let op = "permissions.contexts"
    struct Reply: Decodable, Sendable { let contexts: [PermissionContext] }
}

enum PermissionAction: String, Encodable, Sendable { case start, status, complete, cancel, enroll }

struct TingPermissionRequest: UIRequest {
    static let op = "permissions.ting"
    let homeID: String
    let contextID: String
    let action: PermissionAction
    let code: String?
    enum CodingKeys: String, CodingKey { case homeID = "home_id", contextID = "context_id", action, code }
    struct Reply: Decodable, Sendable {
        let contextID: String
        let request: TingPermission?
        let enrolled: Bool
        enum CodingKeys: String, CodingKey { case contextID = "context_id", request, enrolled }
    }
}

struct TingPermission: Codable, Sendable, Equatable {
    let requestID: UUID
    let authorization: Authorization
    let completed: Bool
    enum CodingKeys: String, CodingKey { case requestID = "request_id", authorization, completed }
    struct Authorization: Codable, Sendable, Equatable {
        let id: UUID
        let appID: String
        let actor: PermissionActor
        let orgID: String
        let status: String
        let version: Int
        let expiresAt: String
        let authorizationURL: String?
        let state: String?
        enum CodingKeys: String, CodingKey {
            case id, appID = "app_id", actor, orgID = "org_id", status, version
            case expiresAt = "expires_at", authorizationURL = "authorization_url", state
        }
        var reviewURL: URL? {
            guard let authorizationURL,
                  let parts = URLComponents(string: authorizationURL), parts.scheme?.lowercased() == "https",
                  let host = parts.host, !host.isEmpty, parts.user == nil, parts.password == nil
            else { return nil }
            return parts.url
        }
        var expiry: Date? {
            let formatter = ISO8601DateFormatter()
            if let date = formatter.date(from: expiresAt) { return date }
            formatter.formatOptions.insert(.withFractionalSeconds)
            return formatter.date(from: expiresAt)
        }
    }
    func matches(_ context: PermissionContext, previous: Self?) -> Bool {
        let a = authorization
        let nilUUID = UUID(uuid: (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0))
        return requestID != nilUUID && a.id != nilUUID && a.appID == "peek"
            && a.actor == context.actor && a.orgID == context.orgID && a.version > 0
            && a.expiry != nil && (a.authorizationURL == nil || a.reviewURL != nil)
            && ["pending", "approved", "declined", "exchanged", "expired"].contains(a.status)
            && (!completed || ["approved", "exchanged"].contains(a.status))
            && (previous == nil || (previous?.requestID == requestID
                && previous?.authorization.id == a.id && previous?.authorization.state == a.state))
    }
}

@MainActor @Observable
final class PermissionsModel {
    private(set) var contexts: [PermissionContext] = []
    private(set) var selectedID: String?
    private(set) var request: TingPermission?
    private(set) var enrolled = false
    private(set) var busy: PermissionAction?
    private(set) var loadingContexts = false
    private(set) var error: String?
    private(set) var notice: String?
    var code = ""
    @ObservationIgnored private let link: any DaemonLinking
    @ObservationIgnored private var epoch = UUID()
    @ObservationIgnored private var listEpoch = UUID()

    init(link: any DaemonLinking) { self.link = link }
    var selected: PermissionContext? { contexts.first { $0.id == selectedID } }
    var needsFreshReview: Bool {
        guard let request, !request.completed else { return false }
        return ["expired", "declined"].contains(request.authorization.status)
            || (request.authorization.expiry.map { $0 <= Date() } ?? true)
    }

    func select(_ id: String?) {
        guard id != selectedID else { return }
        selectedID = contexts.first { $0.id == id }?.id
        resetTransient()
    }

    func suspend() {
        listEpoch = UUID()
        loadingContexts = false
        resetTransient()
    }

    private func resetTransient() {
        epoch = UUID()
        code = ""
        request = nil
        enrolled = false
        busy = nil
        error = nil
        notice = nil
    }

    func reloadContexts() async {
        let ticket = UUID()
        listEpoch = ticket
        loadingContexts = true
        error = nil
        do throws(DaemonLinkError) {
            let reply = try await link.send(PermissionContextsRequest())
            guard ticket == listEpoch, !Task.isCancelled else { return }
            let previous = selected
            guard Set(reply.contexts.map(\.id)).count == reply.contexts.count,
                  reply.contexts.allSatisfy({ !$0.homeID.isEmpty && !$0.contextID.isEmpty }) else {
                throw DaemonLinkError.invalidReply(op: PermissionContextsRequest.op, reason: "duplicate or missing context")
            }
            contexts = reply.contexts
            let next = contexts.first { $0.id == selectedID } ?? contexts.first
            if previous != next { resetTransient() }
            selectedID = next?.id
            loadingContexts = false
            await perform(.status)
        } catch {
            guard ticket == listEpoch, !Task.isCancelled else { return }
            loadingContexts = false
            self.error = Self.message(for: error)
        }
    }

    func perform(_ action: PermissionAction, expectedID: String? = nil) async {
        guard let context = selected, busy == nil, expectedID == nil || expectedID == context.id else { return }
        guard action != .enroll || request?.completed == true else { return }
        let ticket = epoch
        let previous = request
        let submitted = action == .complete ? code.trimmingCharacters(in: .whitespacesAndNewlines) : ""
        // Keep no extra copy after dispatch. peekd owns uncertain completion recovery.
        code = ""
        busy = action
        error = nil
        notice = nil
        defer { if ticket == epoch { busy = nil } }
        do throws(DaemonLinkError) {
            let reply = try await link.send(TingPermissionRequest(
                homeID: context.homeID, contextID: context.contextID, action: action,
                code: submitted.isEmpty ? nil : submitted))
            guard ticket == epoch, selected == context, !Task.isCancelled else { return }
            guard reply.contextID == context.contextID,
                  reply.request?.matches(context, previous: action == .cancel ? nil : previous) ?? true,
                  action != .complete || reply.request?.completed == true,
                  action != .enroll || (reply.enrolled && reply.request?.completed == true),
                  action != .start || reply.request != nil,
                  action != .cancel || reply.request == nil else {
                throw DaemonLinkError.invalidReply(op: TingPermissionRequest.op, reason: "approval context did not match")
            }
            request = reply.request
            enrolled = reply.enrolled
            if action == .complete {
                notice = "Approval saved. Enable deliveries when you are ready to retry queued answers."
            } else if action == .enroll {
                notice = "Deliveries enabled. Queued answers for this account and organization can now retry."
            } else if action == .cancel {
                notice = "Local review cleared. Your queued answers are retained."
            }
        } catch {
            guard ticket == epoch, selected == context, !Task.isCancelled else { return }
            if case .remote(_, let body) = error,
               ["reconsent_required", "session_rejected", "not_logged_in", "environment_changed"].contains(body.code) {
                request = nil
                if body.code != "reconsent_required" { enrolled = false }
            }
            self.error = Self.message(for: error)
        }
    }

    private static func message(for error: DaemonLinkError) -> String {
        switch error {
        case .remote(_, let body):
            switch body.code {
            case "reconsent_required": "Permissions changed or expired. Start a fresh review. Your queued answers are retained."
            case "session_rejected", "not_logged_in", "environment_changed": "This saved login changed. Refresh accounts or sign in again with the Peek CLI."
            case "unknown_op": "Update peekd to manage permissions here."
            case "invalid_input": "Start a permission review, or enter the approval code from IAM."
            default: "The permission request could not finish. Check its status or retry the same action."
            }
        case .notConnected: "Peek is waiting for its background service. Try again when peekd is connected."
        case .invalidReply, .invalidRequest: "The permission response did not match this account and review. Refresh its status before continuing."
        case .disconnected, .timedOut: "The reply was interrupted. Retry the same action to recover its saved result."
        }
    }
}
