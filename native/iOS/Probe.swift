import SwiftUI

/// Temporary: proves the core starts and lists books in the Simulator.
@main
struct ProbeApp: App {
    @State private var lines: [String] = ["starting…"]

    var body: some Scene {
        WindowGroup {
            List(lines, id: \.self) { Text($0) }
                .task {
                    do {
                        let core = try CoreClient.start(events: EventHub())
                        var books: [BookMeta]?
                        var failure = ""
                        for _ in 0..<80 where books == nil {
                            do { books = try await core.listBooks() } catch {
                                failure = "\(error)"
                                try? await Task.sleep(for: .milliseconds(250))
                            }
                        }
                        lines = books.map { ["books: \($0.count)"] + $0.map(\.title) } ?? ["failed: \(failure)"]
                    } catch {
                        lines = ["start failed: \(error)"]
                    }
                }
        }
    }
}
