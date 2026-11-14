import XCTest

/// Drives the real app on a seeded library (`FLTS_UITEST_CONFIG_DIR`); skipped otherwise.
@MainActor
final class FLTSUITests: XCTestCase {
    private func launch() throws -> XCUIApplication {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_UITEST_CONFIG_DIR"] else {
            throw XCTSkip("set FLTS_UITEST_CONFIG_DIR to a seeded config dir")
        }
        let app = XCUIApplication()
        app.launchEnvironment = ["FLTS_CONFIG_DIR": dir, "FLTS_DISABLE_SYNC": "1", "FLTS_DISABLE_KEYRING": "1"]
        app.launch()
        return app
    }

    func testOpenBookAndFlipPages() throws {
        let app = try launch()
        let book = app.outlines.staticTexts["Lluvia en la ciudad"].firstMatch
        XCTAssertTrue(book.waitForExistence(timeout: 15))
        book.click()
        let page = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        XCTAssertTrue(page.waitForExistence(timeout: 10))
        let before = page.label
        page.click()
        page.typeKey(.rightArrow, modifierFlags: [])
        let flipped = XCTNSPredicateExpectation(predicate: NSPredicate(format: "label != %@", before), object: page)
        XCTAssertEqual(XCTWaiter().wait(for: [flipped], timeout: 5), .completed)
    }

    func testSettingsOpen() throws {
        let app = try launch()
        XCTAssertTrue(app.windows.firstMatch.waitForExistence(timeout: 15))
        app.typeKey(",", modifierFlags: .command)
        XCTAssertTrue(app.windows["Translation"].waitForExistence(timeout: 5) || app.tabs["Translation"].waitForExistence(timeout: 5)
            || app.buttons["Translation"].waitForExistence(timeout: 5))
    }
}

/// Walks the main surfaces, attaching a screenshot of each to the result bundle.
@MainActor
final class FLTSScreenshotTour: XCTestCase {
    func testTour() throws {
        let env = ProcessInfo.processInfo.environment
        guard let dir = env["FLTS_UITEST_CONFIG_DIR"] else {
            throw XCTSkip("set FLTS_UITEST_CONFIG_DIR")
        }
        let app = XCUIApplication()
        app.launchEnvironment = ["FLTS_CONFIG_DIR": dir, "FLTS_DISABLE_SYNC": "1", "FLTS_DISABLE_KEYRING": "1"]
        app.launch()
        // The runner is sandboxed: screenshots go into the result bundle.
        func shot(_ name: String) {
            let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
            attachment.name = name
            attachment.lifetime = .keepAlways
            add(attachment)
        }
        let book = app.outlines.staticTexts["Lluvia en la ciudad"].firstMatch
        XCTAssertTrue(book.waitForExistence(timeout: 15))
        book.click()
        let page = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        XCTAssertTrue(page.waitForExistence(timeout: 10))
        sleep(1)
        // Click two words near the top of the first column.
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.2, dy: 0.1)).click()
        sleep(1)
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.35, dy: 0.18)).click()
        sleep(2)
        shot("1-reader-word")
        page.typeKey(.rightArrow, modifierFlags: [])
        sleep(1)
        shot("2-flipped")
        // Drag-select a few words.
        let from = page.coordinate(withNormalizedOffset: CGVector(dx: 0.15, dy: 0.3))
        from.click(forDuration: 0.2, thenDragTo: page.coordinate(withNormalizedOffset: CGVector(dx: 0.6, dy: 0.32)))
        sleep(1)
        shot("3-selection")

        app.typeKey("o", modifierFlags: .command)
        sleep(2)
        shot("4-import")
        app.typeKey(.escape, modifierFlags: [])
        sleep(1)

        app.typeKey(",", modifierFlags: .command)
        sleep(2)
        for tab in ["Translation", "Reading", "Sync", "Anki", "Spotify", "Storage"] {
            let button = app.toolbars.buttons[tab].firstMatch
            if button.exists { button.click() }
            sleep(1)
            shot("5-settings-\(tab)")
        }
        app.typeKey("w", modifierFlags: .command)
        sleep(1)
        app.typeKey("l", modifierFlags: [.command, .shift])
        sleep(3)
        shot("6-lyrics")
    }
}
