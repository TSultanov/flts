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
        // The book may sit in a collapsed folder.
        if app.disclosureTriangles.firstMatch.waitForExistence(timeout: 5) {
            for triangle in app.disclosureTriangles.allElementsBoundByIndex where triangle.value as? Int == 0 {
                triangle.click()
            }
        }
        let book = app.staticTexts["Lluvia en la ciudad"].firstMatch
        XCTAssertTrue(book.waitForExistence(timeout: 15))
        book.doubleClick()
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

/// Walks the main surfaces, asserting focus and attaching screenshots to the
/// result bundle (the runner is sandboxed and cannot write files).
@MainActor
final class FLTSScreenshotTour: XCTestCase {
    private func launch(dir: String, dark: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["FLTS_CONFIG_DIR": dir, "FLTS_DISABLE_SYNC": "1", "FLTS_DISABLE_KEYRING": "1"]
        app.launchArguments = ["-AppleInterfaceStyle", dark ? "Dark" : "Light", "-ApplePersistenceIgnoreState", "YES"]
        app.launch()
        return app
    }

    private func shot(_ element: XCUIElement, _ name: String) {
        let attachment = XCTAttachment(screenshot: element.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// The page label must change: proves keys still reach the reader.
    private func assertFlips(_ app: XCUIApplication, _ context: String) {
        let page = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        let before = page.label
        // "Pages a to b of n": on the last spread only going back can change it.
        let numbers = before.split(whereSeparator: { !$0.isNumber }).compactMap { Int($0) }
        let atEnd = numbers.count >= 2 && numbers[numbers.count - 2] >= numbers[numbers.count - 1]
        app.typeKey(atEnd ? .leftArrow : .rightArrow, modifierFlags: [])
        let changed = XCTNSPredicateExpectation(predicate: NSPredicate(format: "label != %@", before), object: page)
        XCTAssertEqual(XCTWaiter().wait(for: [changed], timeout: 3), .completed, "arrow key ignored \(context)")
    }

    func testTour() throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_UITEST_CONFIG_DIR"] else {
            throw XCTSkip("set FLTS_UITEST_CONFIG_DIR")
        }
        let app = launch(dir: dir)
        let window = app.windows.firstMatch
        let book = app.staticTexts["Lluvia en la ciudad"].firstMatch
        XCTAssertTrue(book.waitForExistence(timeout: 15))
        sleep(1)
        shot(window, "0-library")

        book.doubleClick()
        let page = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        XCTAssertTrue(page.waitForExistence(timeout: 10))
        sleep(2)
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.12)).click()
        sleep(1)
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.55, dy: 0.2)).click()
        sleep(2)
        shot(window, "1-reader-word")
        assertFlips(app, "after tapping words")

        app.typeKey("w", modifierFlags: [])
        sleep(1)
        shot(window, "2-word-expanded")
        app.buttons["Hide word details"].firstMatch.click()
        sleep(1)
        assertFlips(app, "after clicking in the word panel")

        app.typeKey("c", modifierFlags: [])
        sleep(1)
        shot(window, "3-chapters")
        app.outlines.firstMatch.cells.firstMatch.click()
        sleep(1)
        assertFlips(app, "after picking a chapter")
        app.typeKey("c", modifierFlags: [])
        sleep(1)

        let from = page.coordinate(withNormalizedOffset: CGVector(dx: 0.15, dy: 0.3))
        from.click(forDuration: 0.2, thenDragTo: page.coordinate(withNormalizedOffset: CGVector(dx: 0.6, dy: 0.32)))
        sleep(1)
        shot(window, "4-selection")

        app.buttons["Library"].firstMatch.click()
        sleep(1)
        // Move the book into a new folder through the UI.
        book.rightClick()
        app.menuItems["Move to Folder…"].click()
        let sheet = app.sheets.firstMatch
        XCTAssertTrue(sheet.waitForExistence(timeout: 5))
        sheet.textFields.firstMatch.click()
        sheet.typeText("Cuentos")
        sheet.buttons["Move"].click()
        sleep(2)
        let folder = app.descendants(matching: .any).matching(NSPredicate(format: "label == 'Cuentos' OR title == 'Cuentos' OR value == 'Cuentos'")).firstMatch
        XCTAssertTrue(folder.waitForExistence(timeout: 5), "folder row after moving the book")
        app.disclosureTriangles.firstMatch.click()
        sleep(1)
        shot(window, "6-library-folder")

        app.typeKey("o", modifierFlags: .command)
        sleep(2)
        shot(window, "7-import")
        app.typeKey(.escape, modifierFlags: [])
        sleep(1)

        app.buttons["Lyrics"].firstMatch.click()
        let lyrics = app.windows["Lyrics"]
        XCTAssertTrue(lyrics.waitForExistence(timeout: 5))
        sleep(3)
        shot(lyrics, "8-lyrics")
        lyrics.buttons[XCUIIdentifierCloseWindow].click()

        app.typeKey(",", modifierFlags: .command)
        sleep(2)
        for tab in ["Translation", "Sync", "Spotify"] {
            let button = app.toolbars.buttons[tab].firstMatch
            if button.exists { button.click() }
            sleep(1)
            shot(app.windows.firstMatch, "9-settings-\(tab)")
        }
        app.terminate()

        // The reader in dark mode.
        let dark = launch(dir: dir, dark: true)
        XCTAssertTrue(dark.disclosureTriangles.firstMatch.waitForExistence(timeout: 15))
        dark.disclosureTriangles.firstMatch.click()
        let darkBook = dark.staticTexts["Lluvia en la ciudad"].firstMatch
        XCTAssertTrue(darkBook.waitForExistence(timeout: 5))
        darkBook.doubleClick()
        sleep(3)
        dark.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
            .coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.12)).click()
        sleep(2)
        shot(dark.windows.firstMatch, "10-dark-reader")
        dark.terminate()

        // First launch: onboarding on an empty config dir.
        if let empty = ProcessInfo.processInfo.environment["FLTS_UITEST_EMPTY_DIR"] {
            let first = launch(dir: empty)
            XCTAssertTrue(first.staticTexts["Welcome to FLTS"].waitForExistence(timeout: 15))
            sleep(1)
            shot(first.windows.firstMatch, "11-onboarding")
            first.terminate()
        }
    }
}
