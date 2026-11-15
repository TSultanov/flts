import XCTest

/// Polls quickly: XCTest's own waits re-check only once a second.
@MainActor
private func until(_ timeout: Double, _ condition: () -> Bool) -> Bool {
    let end = Date().addingTimeInterval(timeout)
    repeat {
        if condition() { return true }
        Thread.sleep(forTimeInterval: 0.05)
    } while Date() < end
    return false
}

extension XCUIElement {
    @MainActor
    fileprivate func appears(timeout: Double) -> Bool {
        until(timeout) { exists }
    }
}

/// Drives the iOS app in the Simulator on a seeded library
/// (`FLTS_UITEST_CONFIG_DIR`, a host folder), attaching screenshots.
@MainActor
final class FLTSiOSTour: XCTestCase {
    private func launch(dir: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = [
            "FLTS_CONFIG_DIR": dir, "FLTS_DISABLE_SYNC": "1", "FLTS_DISABLE_KEYRING": "1", "FLTS_NO_ANIMATIONS": "1",
        ]
        app.launch()
        return app
    }

    private func pause(_ seconds: Double = 0.3) {
        Thread.sleep(forTimeInterval: seconds)
    }

    private func shot(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func page(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
    }

    func testTour() throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_UITEST_CONFIG_DIR"], !dir.isEmpty else {
            throw XCTSkip("set FLTS_UITEST_CONFIG_DIR")
        }
        let app = launch(dir: dir)
        let book = app.staticTexts["Lluvia en la ciudad"].firstMatch
        XCTAssertTrue(book.appears(timeout: 30), "the library did not list the book")
        pause()
        shot(app, "0-library")

        // A tap opens the book.
        book.tap()
        let page = page(app)
        XCTAssertTrue(page.appears(timeout: 15), "the book did not open")
        XCTAssertTrue(until(20) { page.label.contains(" of ") }, "the chapter never finished laying out")
        pause()
        shot(app, "1-reader")

        // A tap on a word shows its translation; the bar's button opens the details.
        let details = app.buttons["Show word details"].firstMatch
        var found = false
        for step in 0..<16 where !found {
            page.coordinate(withNormalizedOffset: CGVector(dx: 0.25 + 0.1 * Double(step % 3), dy: 0.08 + 0.04 * Double(step / 3))).tap()
            found = until(1.5) { details.exists && details.isEnabled }
        }
        XCTAssertTrue(found, "no translated word could be selected")
        shot(app, "2-word")
        XCTAssertTrue(app.buttons["Show system dictionary"].firstMatch.exists, "no system dictionary button")
        details.tap()
        XCTAssertTrue(app.buttons["Hide word details"].firstMatch.appears(timeout: 3), "the word's details did not open")
        pause()
        shot(app, "3-word-details")
        app.buttons["Hide word details"].firstMatch.tap()

        // Landscape: an iPad shows two pages side by side.
        XCUIDevice.shared.orientation = .landscapeLeft
        pause(1.5)
        XCTAssertTrue(until(20) { page.label.contains(" of ") })
        shot(app, "3-landscape")
        if UIDevice.current.userInterfaceIdiom == .pad {
            XCTAssertTrue(page.label.contains(" to "), "no two-page spread on an iPad in landscape: \(page.label)")
        }
        XCUIDevice.shared.orientation = .portrait
        pause(1.5)
        XCTAssertTrue(until(20) { page.label.contains(" of ") })

        // One swipe turns one spread, either way.
        let before = page.label
        page.swipeLeft()
        XCTAssertTrue(until(3) { page.label != before }, "a swipe did not turn the page")
        let after = page.label
        page.swipeRight()
        XCTAssertTrue(until(3) { page.label == before }, "a swipe back did not return: \(after) → \(page.label)")

        // Back to the library; a book with chapters has a Chapters button.
        app.buttons["Library"].firstMatch.tap()
        let chaptered = app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos breves'")).firstMatch
        XCTAssertTrue(chaptered.appears(timeout: 10))
        chaptered.tap()
        XCTAssertTrue(self.page(app).appears(timeout: 15))
        let chapters = app.buttons["Chapters"].firstMatch
        XCTAssertTrue(chapters.appears(timeout: 5), "no Chapters button on a book with chapters")
        chapters.tap()
        let uno = app.staticTexts["Uno"].firstMatch
        XCTAssertTrue(uno.appears(timeout: 5), "the chapter list did not open")
        pause()
        shot(app, "4-chapters")
        uno.tap()
        XCTAssertTrue(until(5) { !app.staticTexts["Dos"].exists }, "the chapter list did not close after a pick")
        pause()
        shot(app, "5-chapter-open")
        app.buttons["Library"].firstMatch.tap()

        // Settings is a sheet with a page per section.
        let settings = app.buttons["Settings"].firstMatch
        XCTAssertTrue(settings.appears(timeout: 10))
        settings.tap()
        XCTAssertTrue(app.staticTexts["Translation"].firstMatch.appears(timeout: 5), "Settings did not open")
        pause()
        shot(app, "6-settings")
        app.staticTexts["Reading"].firstMatch.tap()
        XCTAssertTrue(app.sliders.firstMatch.appears(timeout: 5), "no text size slider")
        pause()
        shot(app, "7-settings-reading")
        app.navigationBars["Reading"].buttons.firstMatch.tap()
        XCTAssertTrue(app.staticTexts["Translation"].firstMatch.appears(timeout: 5), "Back did not return to the settings list")
        app.staticTexts["Translation"].firstMatch.tap()
        pause()
        shot(app, "8-settings-translation")
        app.navigationBars["Translation"].buttons.firstMatch.tap()
        XCTAssertTrue(app.staticTexts["Sync"].firstMatch.appears(timeout: 5))
        app.staticTexts["Sync"].firstMatch.tap()
        pause()
        shot(app, "9-settings-sync")
        app.navigationBars["Sync"].buttons.firstMatch.tap()
        XCTAssertTrue(app.buttons["Done"].firstMatch.appears(timeout: 5))
        app.buttons["Done"].firstMatch.tap()

        // The import sheet.
        app.buttons["Import"].firstMatch.tap()
        XCTAssertTrue(app.buttons["Choose EPUB…"].firstMatch.appears(timeout: 5), "the import sheet did not open")
        pause()
        shot(app, "10-import")
        app.buttons["Text"].firstMatch.tap()
        pause()
        shot(app, "11-import-text")
        app.buttons["Cancel"].firstMatch.tap()
        app.terminate()

        // First launch: onboarding on an empty config dir, finished with a key.
        if let empty = ProcessInfo.processInfo.environment["FLTS_UITEST_EMPTY_DIR"], !empty.isEmpty {
            let first = launch(dir: empty)
            XCTAssertTrue(first.staticTexts["Welcome to FLTS"].firstMatch.appears(timeout: 30), "no onboarding on an empty config dir")
            pause()
            shot(first, "12-onboarding")
            let key = first.secureTextFields.firstMatch
            XCTAssertTrue(key.appears(timeout: 5), "no API key field")
            key.tap()
            key.typeText("test-key\n")
            let empty = first.staticTexts["Your Library Is Empty"].firstMatch
            if !empty.appears(timeout: 3) { first.buttons["Continue"].firstMatch.tap() }
            XCTAssertTrue(empty.appears(timeout: 10), "onboarding did not lead to the library")
            pause()
            shot(first, "13-empty-library")
            first.terminate()
        }
    }
}
