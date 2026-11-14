import Foundation
import os

let log = Logger(subsystem: "com.TS.FLTS", category: "app")

/// A failed core command, carrying the backend's message.
struct CoreFailure: LocalizedError, Sendable {
    let message: String
    var errorDescription: String? { message }
}

/// What stores need from the core; faked in tests.
protocol CoreService: Sendable {
    func invokeRaw(_ cmd: String, _ args: [String: JSONValue]) async throws -> Data
}

extension CoreService {
    func invoke<T: Decodable>(
        _ cmd: String, _ args: [String: JSONValue] = [:], as type: T.Type = T.self
    ) async throws -> T {
        let data = try await invokeRaw(cmd, args)
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            log.error("decoding \(cmd, privacy: .public) failed: \(error, privacy: .public)")
            throw CoreFailure(message: "Unexpected reply to \(cmd): \(error)")
        }
    }

    func call(_ cmd: String, _ args: [String: JSONValue] = [:]) async throws {
        _ = try await invokeRaw(cmd, args)
    }
}

/// The Rust core behind UniFFI. Events arrive on Rust threads and are hopped
/// onto the main actor before reaching stores.
final class CoreClient: CoreService {
    private let core: FltsCore
    let events: EventHub

    @MainActor
    init(events: EventHub) throws {
        self.events = events
        let shell = ShellBridge(events: events)
        do {
            core = try FltsCore.start(shell: shell)
        } catch {
            throw CoreFailure(message: "\(error)")
        }
    }

    func invokeRaw(_ cmd: String, _ args: [String: JSONValue]) async throws -> Data {
        let json = try JSONEncoder().encode(args)
        do {
            let reply = try await core.invoke(cmd: cmd, args: String(decoding: json, as: UTF8.self))
            return Data(reply.utf8)
        } catch let CoreError.Failed(message) {
            throw CoreFailure(message: message)
        }
    }

    func shutdown() async {
        await core.shutdown()
    }
}

private final class ShellBridge: Shell {
    let events: EventHub

    init(events: EventHub) {
        self.events = events
    }

    func onEvent(name: String, payload: String) {
        let data = Data(payload.utf8)
        Task { @MainActor [events] in
            events.deliver(name, data)
        }
    }

    func runOnMain(task: MainTask) {
        DispatchQueue.main.async {
            task.run()
        }
    }
}
