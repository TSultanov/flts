import Foundation

/// UI state kept by this app (selected book, reader size, exact positions).
/// A run on another config dir, or under tests, keeps its own suite so it
/// never touches the real app's state.
enum AppDefaults {
    static var store: UserDefaults {
        let env = ProcessInfo.processInfo.environment
        let isolated = !(env["FLTS_CONFIG_DIR"] ?? "").isEmpty || env["XCTestConfigurationFilePath"] != nil
        return isolated ? UserDefaults(suiteName: "com.TS.FLTS.isolated") ?? .standard : .standard
    }
}
