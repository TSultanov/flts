import Testing

@testable import FLTS

@MainActor
struct ReaderPreferencesTests {
    /// Out-of-range sizes clamp without re-entering the observer forever.
    @Test func fontSizeClamps() {
        let prefs = ReaderPreferences.shared
        let before = prefs.fontSize
        defer { prefs.fontSize = before }
        prefs.fontSize = 100
        #expect(prefs.fontSize == 36)
        prefs.fontSize = 1
        #expect(prefs.fontSize == 12)
        prefs.fontSize = 20
        #expect(prefs.fontSize == 20)
    }
}
