import Foundation

/// Fans core events out to subscribers by name.
@MainActor
final class EventHub {
    struct Token: Hashable {
        fileprivate let id: Int
    }

    private var handlers: [String: [Int: (Data) -> Void]] = [:]
    private var names: [Int: String] = [:]
    private var next = 0

    nonisolated init() {}

    @discardableResult
    func on(_ name: String, _ handler: @escaping (Data) -> Void) -> Token {
        next += 1
        handlers[name, default: [:]][next] = handler
        names[next] = name
        return Token(id: next)
    }

    /// Decodes the payload; payloads that do not decode are dropped and logged.
    @discardableResult
    func on<T: Decodable>(_ name: String, as type: T.Type, _ handler: @escaping (T) -> Void) -> Token {
        on(name) { data in
            do {
                handler(try JSONDecoder().decode(T.self, from: data))
            } catch {
                log.error("event \(name, privacy: .public) did not decode: \(error, privacy: .public)")
            }
        }
    }

    func cancel(_ token: Token) {
        guard let name = names.removeValue(forKey: token.id) else { return }
        handlers[name]?[token.id] = nil
    }

    func cancel(_ tokens: [Token]) {
        tokens.forEach(cancel)
    }

    func deliver(_ name: String, _ payload: Data) {
        guard let list = handlers[name] else { return }
        for handler in list.values {
            handler(payload)
        }
    }
}
