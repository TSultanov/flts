import Foundation

/// UI state kept by this app (selected book, reader size, exact positions).
/// A run on another config dir, or under tests, keeps its own suite so it
/// never touches the real app's state.
enum AppDefaults {
    /// UI tests turn animations off (with a test config dir): each one makes
    /// the test runner wait for the app to settle.
    static let animationsOff: Bool = {
        let env = ProcessInfo.processInfo.environment
        return env["FLTS_NO_ANIMATIONS"] != nil && !(env["FLTS_CONFIG_DIR"] ?? "").isEmpty
    }()

    static var store: UserDefaults {
        let env = ProcessInfo.processInfo.environment
        let isolated = !(env["FLTS_CONFIG_DIR"] ?? "").isEmpty || env["XCTestConfigurationFilePath"] != nil
        return isolated ? UserDefaults(suiteName: "com.TS.FLTS.isolated") ?? .standard : .standard
    }
}
