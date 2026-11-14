import Foundation

/// Fans core events out to subscribers.
@MainActor
final class EventHub {
    struct Token: Hashable {
        fileprivate let id: Int
    }

    private var handlers: [Int: (CoreEvent) -> Void] = [:]
    private var next = 0

    nonisolated init() {}

    @discardableResult
    func on(_ handler: @escaping (CoreEvent) -> Void) -> Token {
        next += 1
        handlers[next] = handler
        return Token(id: next)
    }

    func cancel(_ token: Token) {
        handlers[token.id] = nil
    }

    func cancel(_ tokens: [Token]) {
        tokens.forEach(cancel)
    }

    func deliver(_ event: CoreEvent) {
        for handler in handlers.values {
            handler(event)
        }
    }
}
