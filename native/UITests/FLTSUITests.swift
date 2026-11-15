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
    private func launch(dir: String, dark: Bool = false, smallest: Bool = false, defaultSize: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment = ["FLTS_CONFIG_DIR": dir, "FLTS_DISABLE_SYNC": "1", "FLTS_DISABLE_KEYRING": "1"]
        // Only the dark launch pins the setting: an argument outranks what the app saves.
        app.launchArguments = ["-ApplePersistenceIgnoreState", "YES"] + (dark ? ["-appearance", "dark"] : [])
        if smallest {
            // Below the minimum: the window opens at its smallest allowed size.
            app.launchArguments += ["-NSWindow Frame main-AppWindow-1", "100 450 400 300 0 0 1728 1083 "]
        } else if defaultSize {
            // The scene's default size; otherwise AppKit reuses the last saved frame.
            app.launchArguments += ["-NSWindow Frame main-AppWindow-1", "100 150 1180 800 0 0 1728 1083 "]
        }
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
        app.typeKey("w", modifierFlags: [])
        // A click on empty page space deselects the word.
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.985)).click()
        let hint = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Select a word' OR value BEGINSWITH 'Select a word'")).firstMatch
        XCTAssertTrue(hint.waitForExistence(timeout: 3), "clicking empty space did not deselect")
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.55, dy: 0.2)).click()
        sleep(2)
        app.typeKey("w", modifierFlags: [])
        sleep(1)
        app.buttons["Hide word details"].firstMatch.click()
        sleep(1)
        assertFlips(app, "after clicking in the word panel")

        // A one-chapter book has no chapter list (as in the web app), nor a toggle for it.
        app.typeKey("c", modifierFlags: [])
        sleep(1)
        XCTAssertFalse(app.outlines.firstMatch.exists, "a one-chapter book shows a chapter list")
        XCTAssertFalse(window.toolbars.buttons["Show Sidebar"].exists || window.toolbars.buttons["Hide Sidebar"].exists,
            "a one-chapter book has a sidebar toggle")
        assertFlips(app, "after C on a one-chapter book")

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

        // A book with chapters: the list is hidden until C, and closes after a pick.
        let epub = app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos breves' OR value CONTAINS 'Cuentos breves'")).firstMatch
        XCTAssertTrue(epub.waitForExistence(timeout: 5), "the multi-chapter fixture book is missing")
        do {
            epub.doubleClick()
            sleep(2)
            XCTAssertFalse(app.outlines.firstMatch.exists, "chapter list should start hidden")
            app.typeKey("c", modifierFlags: [])
            XCTAssertTrue(app.outlines.firstMatch.waitForExistence(timeout: 3), "C did not show the chapters")
            sleep(1)
            shot(window, "3-chapters")
            app.outlines.firstMatch.cells.element(boundBy: 1).click()
            sleep(1)
            XCTAssertFalse(app.outlines.firstMatch.exists, "the list did not close after picking a chapter")
            // Chapter 2 waits for chapter 1's summary, which this test library never gets.
            let translate = app.buttons["Translate Chapter"].firstMatch
            XCTAssertTrue(translate.exists, "no Translate Chapter button on an untranslated chapter")
            XCTAssertFalse(translate.isEnabled, "translating before the summaries exist")
            app.typeKey("c", modifierFlags: [])
            XCTAssertTrue(app.outlines.firstMatch.waitForExistence(timeout: 3), "C ignored after picking a chapter")
            app.typeKey("c", modifierFlags: [])
            app.buttons["Library"].firstMatch.click()
            sleep(1)
        }

        // Batch selection bar.
        app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos breves' OR value CONTAINS 'Cuentos breves'"))
            .firstMatch.click()
        if app.disclosureTriangles.firstMatch.exists, app.disclosureTriangles.firstMatch.value as? Int == 0 {
            app.disclosureTriangles.firstMatch.click()
        }
        app.typeKey("a", modifierFlags: .command)
        let selected = app.staticTexts.matching(NSPredicate(format: "label ENDSWITH ' selected' OR value ENDSWITH ' selected'")).firstMatch
        XCTAssertTrue(selected.waitForExistence(timeout: 3), "no selection bar for several books")
        shot(window, "5-selection-bar")
        app.typeKey(.escape, modifierFlags: [])

        app.typeKey("o", modifierFlags: .command)
        sleep(2)
        if let epubFile = ProcessInfo.processInfo.environment["FLTS_UITEST_EPUB"] {
            // Pick the file through the open panel, then skip one chapter.
            app.buttons["Choose EPUB…"].firstMatch.click()
            sleep(2)
            app.typeKey("g", modifierFlags: [.command, .shift])
            sleep(1)
            app.typeText(epubFile + "\r")
            sleep(1)
            app.typeKey(.return, modifierFlags: [])
            let boxes = app.sheets.firstMatch.checkBoxes
            XCTAssertTrue(boxes.firstMatch.waitForExistence(timeout: 10), "no chapter choices after picking the EPUB")
            boxes.element(boundBy: 0).click()
            sleep(1)
            shot(window, "7-import")
            app.sheets.firstMatch.buttons["Import"].click()
            let twoChapters = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH '2 chapters' OR value BEGINSWITH '2 chapters'")).firstMatch
            XCTAssertTrue(twoChapters.waitForExistence(timeout: 10), "imported book with the chosen chapters not in the library")
            // Delete it again: select, ⌫, confirm.
            let imported = app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Cuentos nuevos' OR value CONTAINS 'Cuentos nuevos'")).firstMatch
            imported.click()
            app.typeKey(.delete, modifierFlags: [])
            let confirm = app.windows.firstMatch.descendants(matching: .button).matching(identifier: "Delete").firstMatch
            XCTAssertTrue(confirm.waitForExistence(timeout: 5), "no delete confirmation")
            confirm.click()
            let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: imported)
            XCTAssertEqual(XCTWaiter().wait(for: [gone], timeout: 10), .completed, "deleted book still listed")
        } else {
            shot(window, "7-import")
            app.typeKey(.escape, modifierFlags: [])
        }
        sleep(1)

        app.buttons["Lyrics"].firstMatch.click()
        let lyrics = app.windows["Lyrics"]
        XCTAssertTrue(lyrics.waitForExistence(timeout: 5))
        sleep(3)
        shot(lyrics, "8-lyrics")
        lyrics.buttons[XCUIIdentifierCloseWindow].click()

        app.typeKey(",", modifierFlags: .command)
        sleep(2)
        // The Appearance picker, clicked for real.
        app.toolbars.buttons["Reading"].firstMatch.click()
        sleep(1)
        app.radioButtons["Dark"].firstMatch.click()
        sleep(1)
        shot(app.windows.firstMatch, "9-settings-Reading-dark")
        shot(app.windows.element(boundBy: 1), "9-main-window-dark")
        app.radioButtons["Match System"].firstMatch.click()
        sleep(1)
        for tab in ["Translation", "Sync", "Spotify"] {
            let button = app.toolbars.buttons[tab].firstMatch
            if button.exists { button.click() }
            sleep(1)
            shot(app.windows.firstMatch, "9-settings-\(tab)")
        }
        // ⌘Q goes through applicationShouldTerminate (.terminateLater, bounded shutdown).
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 15), "the app did not quit")
        let again = launch(dir: dir)
        XCTAssertTrue(again.descendants(matching: .any).matching(NSPredicate(format: "label == 'Cuentos'")).firstMatch
            .waitForExistence(timeout: 15), "relaunch did not open on the library")
        again.terminate()

        // On screen in dark mode (the app's own appearance setting).
        let dark = launch(dir: dir, dark: true)
        if dark.disclosureTriangles.firstMatch.waitForExistence(timeout: 15),
            dark.disclosureTriangles.firstMatch.value as? Int == 0
        {
            dark.disclosureTriangles.firstMatch.click()
        }
        let darkBook = dark.staticTexts["Lluvia en la ciudad"].firstMatch
        XCTAssertTrue(darkBook.waitForExistence(timeout: 10))
        darkBook.doubleClick()
        let darkPage = dark.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        XCTAssertTrue(darkPage.waitForExistence(timeout: 10))
        sleep(2)
        darkPage.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.12)).click()
        sleep(1)
        dark.typeKey("w", modifierFlags: [])
        sleep(1)
        shot(dark.windows.firstMatch, "10-dark-reader")
        dark.buttons["Lyrics"].firstMatch.click()
        let darkLyrics = dark.windows["Lyrics"]
        if darkLyrics.waitForExistence(timeout: 5) {
            sleep(2)
            shot(darkLyrics, "10-dark-lyrics")
            darkLyrics.buttons[XCUIIdentifierCloseWindow].click()
        }
        dark.terminate()

        // The smallest window, driven from the keyboard alone: arrows and
        // Return open a book without a click.
        let small = launch(dir: dir, smallest: true)
        let smallWindow = small.windows.firstMatch
        XCTAssertTrue(smallWindow.waitForExistence(timeout: 15))
        sleep(2)
        print("SMALLEST-WINDOW \(smallWindow.frame.size)")
        shot(smallWindow, "12-min-library")
        small.typeKey(.downArrow, modifierFlags: [])
        small.typeKey(.downArrow, modifierFlags: [])
        sleep(1)
        shot(smallWindow, "12-min-after-arrows")
        small.typeKey(.return, modifierFlags: [])
        let smallPage = small.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page'")).firstMatch
        XCTAssertTrue(smallPage.waitForExistence(timeout: 10), "arrows and Return did not open a book")
        sleep(2)
        smallPage.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.15)).click()
        sleep(1)
        small.typeKey("w", modifierFlags: [])
        sleep(1)
        shot(smallWindow, "12-min-reader")
        small.terminate()

        // First launch: onboarding on an empty config dir.
        if let empty = ProcessInfo.processInfo.environment["FLTS_UITEST_EMPTY_DIR"] {
            let first = launch(dir: empty, defaultSize: true)
            XCTAssertTrue(first.staticTexts["Welcome to FLTS"].waitForExistence(timeout: 15))
            first.buttons["Continue"].click()
            XCTAssertTrue(first.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Paste an API key' OR value BEGINSWITH 'Paste an API key'")).firstMatch
                .waitForExistence(timeout: 5), "onboarding says nothing about the missing key")
            sleep(1)
            shot(first.windows.firstMatch, "11-onboarding")
            // Finish it: a key, Return, and the app moves on to the (empty) library.
            let key = first.secureTextFields.firstMatch
            key.click()
            key.typeText("test-key\r")
            XCTAssertTrue(first.staticTexts.matching(NSPredicate(format: "label == 'Your Library Is Empty' OR value == 'Your Library Is Empty'"))
                .firstMatch.waitForExistence(timeout: 10), "onboarding did not lead to the library")
            shot(first.windows.firstMatch, "11-after-onboarding")
            first.terminate()
        }
    }
}
