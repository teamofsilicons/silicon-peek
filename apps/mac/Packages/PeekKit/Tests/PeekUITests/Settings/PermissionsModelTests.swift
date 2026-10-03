import Foundation
import PeekCore
import Testing

@testable import PeekUI

private actor PermissionsTestLink: DaemonLinking {
    private(set) var sent: [(String, JSONValue)] = []
    var replies: [Result<JSONValue, DaemonLinkError>] = []
    var pauseNext = false
    private var pending: CheckedContinuation<Result<JSONValue, DaemonLinkError>, Never>?
    func enqueue(_ reply: JSONValue) { replies.append(.success(reply)) }
    func fail(_ error: DaemonLinkError) { replies.append(.failure(error)) }
    func pause() { pauseNext = true }
    func release(_ reply: JSONValue) { pending?.resume(returning: .success(reply)); pending = nil }
    func waiting() -> Bool { pending != nil }
    func start() {}
    func stop() {}
    func events() -> AsyncStream<DaemonEvent> { AsyncStream { $0.finish() } }
    func states() -> AsyncStream<DaemonLinkState> { AsyncStream { $0.finish() } }
    func setRequestHandler(_ handler: (@Sendable (DaemonRequest) async -> DaemonReply)?) {}
    func send<R: UIRequest>(_ request: R, blobs: [Data], timeout: Duration?) async throws(DaemonLinkError) -> R.Reply {
        let fields: [String: JSONValue]
        do { fields = try FrameCoding.fields(of: request) }
        catch { throw .invalidRequest(op: R.op, reason: "fixture encoding failed") }
        sent.append((R.op, .object(fields)))
        let outcome: Result<JSONValue, DaemonLinkError>
        if pauseNext {
            pauseNext = false
            outcome = await withCheckedContinuation { pending = $0 }
        } else if replies.isEmpty {
            throw .invalidReply(op: R.op, reason: "unexpected fixture call")
        } else {
            outcome = replies.removeFirst()
        }
        let value = try outcome.get()
        do { return try FrameCoding.decode(R.Reply.self, from: value) }
        catch { throw .invalidReply(op: R.op, reason: "fixture decoding failed") }
    }
}

@Suite("Native permission context isolation") @MainActor
struct PermissionsModelTests {
    private let actor: JSONValue = ["type": "carbon", "public_id": "ca:alice"]
    private func context(_ name: String = "alpha", home: String = "home-one") -> JSONValue {
        ["home_id": .string(home), "context_id": .string(name), "actor": actor,
         "org_id": .string(name), "api_url": "https://peek.example", "context": "production",
         "label": .string("Alice · " + name)]
    }
    private func reply(_ name: String = "alpha", completed: Bool = false, enrolled: Bool = false,
                       requestID: String = "10000000-0000-4000-8000-000000000001",
                       approvalID: String = "20000000-0000-4000-8000-000000000001",
                       state: String = "original", url: String = "https://iam.example/review",
                       status: String? = nil, empty: Bool = false) -> JSONValue {
        ["context_id": .string(name), "enrolled": .bool(enrolled), "request": empty ? .null : [
            "request_id": .string(requestID), "completed": .bool(completed), "authorization": [
                "id": .string(approvalID), "app_id": "peek", "actor": actor, "org_id": .string(name),
                "status": .string(status ?? (completed ? "exchanged" : "pending")), "version": 1,
                "expires_at": "2099-10-03T10:00:00Z", "authorization_url": .string(url),
                "state": .string(state), "endpoints": []], "roots": []]]
    }
    private func make() async -> (PermissionsModel, PermissionsTestLink) {
        let link = PermissionsTestLink()
        await link.enqueue(["contexts": [context(), context("beta")]])
        await link.enqueue(reply(empty: true))
        let model = PermissionsModel(link: link)
        await model.reloadContexts()
        return (model, link)
    }

    @Test("approval and enabling deliveries require separate explicit actions")
    func explicitEnrollment() async {
        let (model, link) = await make()
        await model.perform(.enroll)
        #expect(await link.sent.count == 2)
        await link.enqueue(reply())
        await model.perform(.start)
        model.code = "  secret approval code  "
        await link.enqueue(reply(completed: true))
        await model.perform(.complete)
        #expect(model.request?.completed == true)
        #expect(!model.enrolled)
        #expect(model.code.isEmpty)
        let completion = await link.sent.last?.1
        #expect(completion?["code"]?.stringValue == "secret approval code")
        #expect(completion?["home_id"]?.stringValue == "home-one")
        #expect(completion?["context_id"]?.stringValue == "alpha")
        #expect(await link.sent.compactMap { $0.1["action"]?.stringValue } == ["status", "start", "complete"])
        await link.enqueue(reply(completed: true, enrolled: true))
        await model.perform(.enroll)
        #expect(model.enrolled)
    }

    @Test("a late completion cannot update another organization or erase its draft")
    func lateCompletion() async {
        let (model, link) = await make()
        await link.enqueue(reply())
        await model.perform(.start)
        model.code = "alpha-code"
        await link.pause()
        let task = Task { await model.perform(.complete) }
        #expect(await settingsSuiteWait { await link.waiting() })
        #expect(model.code.isEmpty)
        model.select(model.contexts[1].id)
        model.code = "beta-code"
        await link.release(reply(completed: true))
        await task.value
        #expect(model.selected?.orgID == "beta")
        #expect(model.request == nil && model.notice == nil && !model.enrolled)
        #expect(model.code == "beta-code")
        #expect(model.busy == nil)
    }

    @Test("closing the pane discards secrets and ignores both list and action replies")
    func disappear() async {
        let (model, link) = await make()
        model.code = "temporary"
        await link.pause()
        let task = Task { await model.perform(.start) }
        #expect(await settingsSuiteWait { await link.waiting() })
        model.suspend()
        await link.release(reply())
        await task.value
        #expect(model.request == nil && model.code.isEmpty && model.busy == nil)
        await link.pause()
        let listTask = Task { await model.reloadContexts() }
        #expect(await settingsSuiteWait { await link.waiting() })
        model.suspend()
        await link.release(["contexts": [context("late")]])
        await listTask.value
        #expect(model.contexts.map(\.orgID) == ["alpha", "beta"])
        #expect(!model.loadingContexts)
    }

    @Test("terms changes clear the old review and completion code")
    func freshReview() async {
        let (model, link) = await make()
        await link.enqueue(reply())
        await model.perform(.start)
        model.code = "old-code"
        await link.fail(.remote(op: TingPermissionRequest.op, IPCErrorBody(code: "reconsent_required", message: "changed")))
        await model.perform(.complete)
        #expect(model.request == nil && model.code.isEmpty)
        #expect(model.error?.contains("fresh review") == true)
        await link.enqueue(reply(requestID: "10000000-0000-4000-8000-000000000002", state: "fresh"))
        await model.perform(.start)
        #expect(model.request?.authorization.state == "fresh")
        #expect(model.error == nil)
    }

    @Test("interrupted completions replay without keeping the code in the UI")
    func recoverCompletion() async {
        let (model, link) = await make()
        await link.enqueue(reply())
        await model.perform(.start)
        model.code = "one-time-code"
        await link.fail(.disconnected(op: TingPermissionRequest.op))
        await model.perform(.complete)
        #expect(model.code.isEmpty && model.request != nil)
        await link.enqueue(reply(completed: true))
        await model.perform(.complete)
        #expect(await link.sent.last?.1["code"] == nil)
        #expect(model.request?.completed == true)
    }

    @Test("identity, receipt, callback state and safe review URL are checked")
    func invalidApprovals() async {
        let bad: [JSONValue] = [
            reply("beta"), reply(requestID: "10000000-0000-4000-8000-000000000002"),
            reply(approvalID: "20000000-0000-4000-8000-000000000002"), reply(state: "changed"),
            reply(url: "http://iam.example/review"), reply(url: "https://user:secret@iam.example/review"),
            reply(completed: true, status: "declined"), reply(requestID: "00000000-0000-0000-0000-000000000000")]
        for value in bad {
            let (model, link) = await make()
            await link.enqueue(reply())
            await model.perform(.start)
            let original = model.request
            await link.enqueue(value)
            await model.perform(.status)
            #expect(model.request == original)
            #expect(model.error != nil)
            #expect(!model.enrolled)
        }
    }

    @Test("only one request per visible context runs at a time")
    func serializedActions() async {
        let (model, link) = await make()
        await link.pause()
        let task = Task { await model.perform(.start) }
        #expect(await settingsSuiteWait { await link.waiting() })
        await model.perform(.start)
        await model.perform(.cancel)
        #expect(await link.sent.count == 3)
        await link.release(reply())
        await task.value
        #expect(model.request != nil)
    }

    @Test("duplicate contexts and same-id changed identities cannot reuse a review")
    func listIdentity() async {
        let (model, link) = await make()
        await link.enqueue(reply())
        await model.perform(.start)
        await link.enqueue(["contexts": [context(), context()]])
        await model.reloadContexts()
        #expect(model.error != nil)
        var changed = context().objectValue!
        changed["api_url"] = "https://other-peek.example"
        await link.enqueue(["contexts": [.object(changed)]])
        await link.enqueue(reply(empty: true))
        await model.reloadContexts()
        #expect(model.request == nil)
        #expect(model.selected?.apiURL == "https://other-peek.example")
    }

    @Test("cancel clears only the local review and preserves enrollment")
    func cancel() async {
        let (model, link) = await make()
        await link.enqueue(reply(completed: true, enrolled: true))
        await model.perform(.status)
        model.code = "discard"
        await link.enqueue(reply(enrolled: true, empty: true))
        await model.perform(.cancel)
        #expect(model.request == nil && model.code.isEmpty && model.enrolled)
    }

    @Test("a queued UI action retains the selection captured at the click")
    func queuedSelection() async {
        let (model, link) = await make()
        let clickedContext = model.selectedID
        model.select(model.contexts[1].id)
        model.code = "beta-draft"
        await model.perform(.start, expectedID: clickedContext)
        #expect(await link.sent.count == 2)
        #expect(model.code == "beta-draft")
        #expect(model.request == nil)
    }
}
