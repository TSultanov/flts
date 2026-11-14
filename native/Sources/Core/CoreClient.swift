import Foundation
import os

let log = Logger(subsystem: "com.TS.FLTS", category: "app")

/// What stores call; the generated `FltsCore` in the app, a fake in tests.
typealias CoreService = FltsCoreProtocol

/// Starts the Rust core with a shell that hops events onto the main actor.
enum CoreClient {
    @MainActor
    static func start(events: EventHub) throws -> FltsCore {
        try FltsCore.start(shell: ShellBridge(events: events))
    }
}

private final class ShellBridge: Shell {
    let events: EventHub

    init(events: EventHub) {
        self.events = events
    }

    /// The main queue keeps events in emission order (separate Tasks would not).
    func onEvent(event: CoreEvent) {
        DispatchQueue.main.async { [events] in
            MainActor.assumeIsolated { events.deliver(event) }
        }
    }

    func runOnMain(task: MainTask) {
        DispatchQueue.main.async {
            task.run()
        }
    }
}
