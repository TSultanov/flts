import Foundation
@testable import FLTS

/// Each method runs its handler if a test set one, else forwards to `inner`
/// (a real core, for counting calls), else throws.
final class FakeCore: FltsCoreProtocol, @unchecked Sendable {
    let inner: (any FltsCoreProtocol)?
    init(inner: (any FltsCoreProtocol)? = nil) { self.inner = inner }
    private let lock = NSLock()
    private var _calls: [String] = []
    var calls: [String] { lock.withLock { _calls } }
    func count(_ name: String) -> Int { calls.filter { $0 == name }.count }
    private func record(_ name: String) { lock.withLock { _calls.append(name) } }

    var createCardForWordHandler: ((Uuid, Number, Number, Number) async throws -> CardResult)?
    func createCardForWord(bookId: Uuid, paragraphId: Number, sentenceId: Number, wordId: Number) async throws  -> CardResult {
        record("createCardForWord")
        guard let h = createCardForWordHandler else {
            if let inner { return try await inner.createCardForWord(bookId: bookId, paragraphId: paragraphId, sentenceId: sentenceId, wordId: wordId) }
            throw CoreError.Failed(message: "not faked: createCardForWord")
        }
        return try await h(bookId, paragraphId, sentenceId, wordId)
    }

    var deleteBookHandler: ((Uuid) async throws -> Void)?
    func deleteBook(bookId: Uuid) async throws {
        record("deleteBook")
        guard let h = deleteBookHandler else {
            if let inner { return try await inner.deleteBook(bookId: bookId) }
            throw CoreError.Failed(message: "not faked: deleteBook")
        }
        return try await h(bookId)
    }

    var getAnkiSyncStatusHandler: (() async throws -> AnkiSyncStatus)?
    func getAnkiSyncStatus() async throws  -> AnkiSyncStatus {
        record("getAnkiSyncStatus")
        guard let h = getAnkiSyncStatusHandler else {
            if let inner { return try await inner.getAnkiSyncStatus() }
            throw CoreError.Failed(message: "not faked: getAnkiSyncStatus")
        }
        return try await h()
    }

    var getBookChapterParagraphIdsHandler: ((Uuid, Number) async throws -> [Number])?
    func getBookChapterParagraphIds(bookId: Uuid, chapterId: Number) async throws  -> [Number] {
        record("getBookChapterParagraphIds")
        guard let h = getBookChapterParagraphIdsHandler else {
            if let inner { return try await inner.getBookChapterParagraphIds(bookId: bookId, chapterId: chapterId) }
            throw CoreError.Failed(message: "not faked: getBookChapterParagraphIds")
        }
        return try await h(bookId, chapterId)
    }

    var getBookReadingStateHandler: ((Uuid) async throws -> BookReadingState?)?
    func getBookReadingState(bookId: Uuid) async throws  -> BookReadingState? {
        record("getBookReadingState")
        guard let h = getBookReadingStateHandler else {
            if let inner { return try await inner.getBookReadingState(bookId: bookId) }
            throw CoreError.Failed(message: "not faked: getBookReadingState")
        }
        return try await h(bookId)
    }

    var getBookSummaryStatusHandler: ((Uuid) async throws -> BookSummaryStatus)?
    func getBookSummaryStatus(bookId: Uuid) async throws  -> BookSummaryStatus {
        record("getBookSummaryStatus")
        guard let h = getBookSummaryStatusHandler else {
            if let inner { return try await inner.getBookSummaryStatus(bookId: bookId) }
            throw CoreError.Failed(message: "not faked: getBookSummaryStatus")
        }
        return try await h(bookId)
    }

    var getConfigHandler: (() async throws -> AppConfig)?
    func getConfig() async throws  -> AppConfig {
        record("getConfig")
        guard let h = getConfigHandler else {
            if let inner { return try await inner.getConfig() }
            throw CoreError.Failed(message: "not faked: getConfig")
        }
        return try await h()
    }

    var getLanguagesHandler: (() throws -> [LanguageInfo])?
    func getLanguages() throws  -> [LanguageInfo] {
        record("getLanguages")
        guard let h = getLanguagesHandler else {
            if let inner { return try inner.getLanguages() }
            throw CoreError.Failed(message: "not faked: getLanguages")
        }
        return try h()
    }

    var getLibraryRootHandler: (() async throws -> String)?
    func getLibraryRoot() async throws  -> String {
        record("getLibraryRoot")
        guard let h = getLibraryRootHandler else {
            if let inner { return try await inner.getLibraryRoot() }
            throw CoreError.Failed(message: "not faked: getLibraryRoot")
        }
        return try await h()
    }

    var getModelsHandler: (() async throws -> [ModelInfo])?
    func getModels() async throws  -> [ModelInfo] {
        record("getModels")
        guard let h = getModelsHandler else {
            if let inner { return try await inner.getModels() }
            throw CoreError.Failed(message: "not faked: getModels")
        }
        return try await h()
    }

    var getNowPlayingHandler: (() async throws -> NowPlaying?)?
    func getNowPlaying() async throws  -> NowPlaying? {
        record("getNowPlaying")
        guard let h = getNowPlayingHandler else {
            if let inner { return try await inner.getNowPlaying() }
            throw CoreError.Failed(message: "not faked: getNowPlaying")
        }
        return try await h()
    }

    var getParagraphTextsHandler: ((Uuid, [Number]) async throws -> ParagraphTexts)?
    func getParagraphTexts(bookId: Uuid, paragraphIds: [Number]) async throws  -> ParagraphTexts {
        record("getParagraphTexts")
        guard let h = getParagraphTextsHandler else {
            if let inner { return try await inner.getParagraphTexts(bookId: bookId, paragraphIds: paragraphIds) }
            throw CoreError.Failed(message: "not faked: getParagraphTexts")
        }
        return try await h(bookId, paragraphIds)
    }

    var getSyncStatusHandler: (() async throws -> SyncStatus)?
    func getSyncStatus() async throws  -> SyncStatus {
        record("getSyncStatus")
        guard let h = getSyncStatusHandler else {
            if let inner { return try await inner.getSyncStatus() }
            throw CoreError.Failed(message: "not faked: getSyncStatus")
        }
        return try await h()
    }

    var getSystemDefinitionHandler: ((String, String, String) async throws -> SystemDefinition?)?
    func getSystemDefinition(word: String, sourceLang: String, targetLang: String) async throws  -> SystemDefinition? {
        record("getSystemDefinition")
        guard let h = getSystemDefinitionHandler else {
            if let inner { return try await inner.getSystemDefinition(word: word, sourceLang: sourceLang, targetLang: targetLang) }
            throw CoreError.Failed(message: "not faked: getSystemDefinition")
        }
        return try await h(word, sourceLang, targetLang)
    }

    var getTrackLyricsStateHandler: ((String, String, String) async throws -> TrackLyricsState)?
    func getTrackLyricsState(trackId: String, targetLang: String, model: String) async throws  -> TrackLyricsState {
        record("getTrackLyricsState")
        guard let h = getTrackLyricsStateHandler else {
            if let inner { return try await inner.getTrackLyricsState(trackId: trackId, targetLang: targetLang, model: model) }
            throw CoreError.Failed(message: "not faked: getTrackLyricsState")
        }
        return try await h(trackId, targetLang, model)
    }

    var getTranslationProvidersHandler: (() throws -> [ProviderMeta])?
    func getTranslationProviders() throws  -> [ProviderMeta] {
        record("getTranslationProviders")
        guard let h = getTranslationProvidersHandler else {
            if let inner { return try inner.getTranslationProviders() }
            throw CoreError.Failed(message: "not faked: getTranslationProviders")
        }
        return try h()
    }

    var getWordInfoHandler: ((Uuid, Number, Number, Number) async throws -> WordInfo?)?
    func getWordInfo(bookId: Uuid, paragraphId: Number, sentenceId: Number, wordId: Number) async throws  -> WordInfo? {
        record("getWordInfo")
        guard let h = getWordInfoHandler else {
            if let inner { return try await inner.getWordInfo(bookId: bookId, paragraphId: paragraphId, sentenceId: sentenceId, wordId: wordId) }
            throw CoreError.Failed(message: "not faked: getWordInfo")
        }
        return try await h(bookId, paragraphId, sentenceId, wordId)
    }

    var importEpubHandler: ((EpubPreview, String) async throws -> Uuid)?
    func importEpub(preview: EpubPreview, sourceLanguageId: String) async throws  -> Uuid {
        record("importEpub")
        guard let h = importEpubHandler else {
            if let inner { return try await inner.importEpub(preview: preview, sourceLanguageId: sourceLanguageId) }
            throw CoreError.Failed(message: "not faked: importEpub")
        }
        return try await h(preview, sourceLanguageId)
    }

    var importPlainTextHandler: ((String, String, String) async throws -> Uuid)?
    func importPlainText(title: String, text: String, sourceLanguageId: String) async throws  -> Uuid {
        record("importPlainText")
        guard let h = importPlainTextHandler else {
            if let inner { return try await inner.importPlainText(title: title, text: text, sourceLanguageId: sourceLanguageId) }
            throw CoreError.Failed(message: "not faked: importPlainText")
        }
        return try await h(title, text, sourceLanguageId)
    }

    var listBookChaptersHandler: ((Uuid) async throws -> [ChapterMeta])?
    func listBookChapters(bookId: Uuid) async throws  -> [ChapterMeta] {
        record("listBookChapters")
        guard let h = listBookChaptersHandler else {
            if let inner { return try await inner.listBookChapters(bookId: bookId) }
            throw CoreError.Failed(message: "not faked: listBookChapters")
        }
        return try await h(bookId)
    }

    var listBooksHandler: (() async throws -> [BookMeta])?
    func listBooks() async throws  -> [BookMeta] {
        record("listBooks")
        guard let h = listBooksHandler else {
            if let inner { return try await inner.listBooks() }
            throw CoreError.Failed(message: "not faked: listBooks")
        }
        return try await h()
    }

    var listTranslationActivityHandler: (() async throws -> [ActiveTranslation])?
    func listTranslationActivity() async throws  -> [ActiveTranslation] {
        record("listTranslationActivity")
        guard let h = listTranslationActivityHandler else {
            if let inner { return try await inner.listTranslationActivity() }
            throw CoreError.Failed(message: "not faked: listTranslationActivity")
        }
        return try await h()
    }

    var moveBookHandler: ((Uuid, [String]) async throws -> Void)?
    func moveBook(bookId: Uuid, path: [String]) async throws {
        record("moveBook")
        guard let h = moveBookHandler else {
            if let inner { return try await inner.moveBook(bookId: bookId, path: path) }
            throw CoreError.Failed(message: "not faked: moveBook")
        }
        return try await h(bookId, path)
    }

    var parseEpubHandler: ((String) async throws -> EpubPreview)?
    func parseEpub(path: String) async throws  -> EpubPreview {
        record("parseEpub")
        guard let h = parseEpubHandler else {
            if let inner { return try await inner.parseEpub(path: path) }
            throw CoreError.Failed(message: "not faked: parseEpub")
        }
        return try await h(path)
    }

    var parseLanguageIdHandler: ((String?) throws -> String?)?
    func parseLanguageId(code: String?)  -> String? {
        record("parseLanguageId")
        if let h = parseLanguageIdHandler { return (try? h(code)) ?? nil }
        return inner?.parseLanguageId(code: code)
    }

    var purgeGeminiCachesHandler: (() async throws -> Number)?
    func purgeGeminiCaches() async throws  -> Number {
        record("purgeGeminiCaches")
        guard let h = purgeGeminiCachesHandler else {
            if let inner { return try await inner.purgeGeminiCaches() }
            throw CoreError.Failed(message: "not faked: purgeGeminiCaches")
        }
        return try await h()
    }

    var saveBookReadingStateHandler: ((Uuid, BookReadingState) async throws -> Void)?
    func saveBookReadingState(bookId: Uuid, state: BookReadingState) async throws {
        record("saveBookReadingState")
        guard let h = saveBookReadingStateHandler else {
            if let inner { return try await inner.saveBookReadingState(bookId: bookId, state: state) }
            throw CoreError.Failed(message: "not faked: saveBookReadingState")
        }
        return try await h(bookId, state)
    }

    var shutdownHandler: (() async throws -> Void)?
    func shutdown() async {
        record("shutdown")
        await inner?.shutdown()
    }

    var spotifyCdpStatusHandler: (() async throws -> SpotifyCdpStatus)?
    func spotifyCdpStatus() async throws  -> SpotifyCdpStatus {
        record("spotifyCdpStatus")
        guard let h = spotifyCdpStatusHandler else {
            if let inner { return try await inner.spotifyCdpStatus() }
            throw CoreError.Failed(message: "not faked: spotifyCdpStatus")
        }
        return try await h()
    }

    var spotifyInstallLoginAgentHandler: (() async throws -> SpotifyLoginAgentStatus)?
    func spotifyInstallLoginAgent() async throws  -> SpotifyLoginAgentStatus {
        record("spotifyInstallLoginAgent")
        guard let h = spotifyInstallLoginAgentHandler else {
            if let inner { return try await inner.spotifyInstallLoginAgent() }
            throw CoreError.Failed(message: "not faked: spotifyInstallLoginAgent")
        }
        return try await h()
    }

    var spotifyLoginAgentStatusHandler: (() async throws -> SpotifyLoginAgentStatus)?
    func spotifyLoginAgentStatus() async throws  -> SpotifyLoginAgentStatus {
        record("spotifyLoginAgentStatus")
        guard let h = spotifyLoginAgentStatusHandler else {
            if let inner { return try await inner.spotifyLoginAgentStatus() }
            throw CoreError.Failed(message: "not faked: spotifyLoginAgentStatus")
        }
        return try await h()
    }

    var spotifyRemoveLoginAgentHandler: (() async throws -> SpotifyLoginAgentStatus)?
    func spotifyRemoveLoginAgent() async throws  -> SpotifyLoginAgentStatus {
        record("spotifyRemoveLoginAgent")
        guard let h = spotifyRemoveLoginAgentHandler else {
            if let inner { return try await inner.spotifyRemoveLoginAgent() }
            throw CoreError.Failed(message: "not faked: spotifyRemoveLoginAgent")
        }
        return try await h()
    }

    var spotifyRestartWithDevtoolsHandler: (() async throws -> SpotifyCdpStatus)?
    func spotifyRestartWithDevtools() async throws  -> SpotifyCdpStatus {
        record("spotifyRestartWithDevtools")
        guard let h = spotifyRestartWithDevtoolsHandler else {
            if let inner { return try await inner.spotifyRestartWithDevtools() }
            throw CoreError.Failed(message: "not faked: spotifyRestartWithDevtools")
        }
        return try await h()
    }

    var spotifyWebConnectHandler: ((String) async throws -> Void)?
    func spotifyWebConnect(clientId: String) async throws {
        record("spotifyWebConnect")
        guard let h = spotifyWebConnectHandler else {
            if let inner { return try await inner.spotifyWebConnect(clientId: clientId) }
            throw CoreError.Failed(message: "not faked: spotifyWebConnect")
        }
        return try await h(clientId)
    }

    var spotifyWebDisconnectHandler: (() async throws -> Void)?
    func spotifyWebDisconnect() async throws {
        record("spotifyWebDisconnect")
        guard let h = spotifyWebDisconnectHandler else {
            if let inner { return try await inner.spotifyWebDisconnect() }
            throw CoreError.Failed(message: "not faked: spotifyWebDisconnect")
        }
        return try await h()
    }

    var spotifyWebQueueHandler: (() async throws -> QueueSnapshot?)?
    func spotifyWebQueue() async throws  -> QueueSnapshot? {
        record("spotifyWebQueue")
        guard let h = spotifyWebQueueHandler else {
            if let inner { return try await inner.spotifyWebQueue() }
            throw CoreError.Failed(message: "not faked: spotifyWebQueue")
        }
        return try await h()
    }

    var spotifyWebStatusHandler: (() async throws -> SpotifyWebStatus)?
    func spotifyWebStatus() async throws  -> SpotifyWebStatus {
        record("spotifyWebStatus")
        guard let h = spotifyWebStatusHandler else {
            if let inner { return try await inner.spotifyWebStatus() }
            throw CoreError.Failed(message: "not faked: spotifyWebStatus")
        }
        return try await h()
    }

    var startSpotifyWatcherHandler: (() async throws -> Void)?
    func startSpotifyWatcher() async throws {
        record("startSpotifyWatcher")
        guard let h = startSpotifyWatcherHandler else {
            if let inner { return try await inner.startSpotifyWatcher() }
            throw CoreError.Failed(message: "not faked: startSpotifyWatcher")
        }
        return try await h()
    }

    var stopSpotifyWatcherHandler: (() async throws -> Void)?
    func stopSpotifyWatcher() async throws {
        record("stopSpotifyWatcher")
        guard let h = stopSpotifyWatcherHandler else {
            if let inner { return try await inner.stopSpotifyWatcher() }
            throw CoreError.Failed(message: "not faked: stopSpotifyWatcher")
        }
        return try await h()
    }

    var syncAddDeviceHandler: ((String, String) async throws -> Void)?
    func syncAddDevice(deviceId: String, name: String) async throws {
        record("syncAddDevice")
        guard let h = syncAddDeviceHandler else {
            if let inner { return try await inner.syncAddDevice(deviceId: deviceId, name: name) }
            throw CoreError.Failed(message: "not faked: syncAddDevice")
        }
        return try await h(deviceId, name)
    }

    var syncAnkiNowHandler: (() async throws -> Void)?
    func syncAnkiNow() async throws {
        record("syncAnkiNow")
        guard let h = syncAnkiNowHandler else {
            if let inner { return try await inner.syncAnkiNow() }
            throw CoreError.Failed(message: "not faked: syncAnkiNow")
        }
        return try await h()
    }

    var syncKeepIdentityHandler: (() async throws -> Void)?
    func syncKeepIdentity() async throws {
        record("syncKeepIdentity")
        guard let h = syncKeepIdentityHandler else {
            if let inner { return try await inner.syncKeepIdentity() }
            throw CoreError.Failed(message: "not faked: syncKeepIdentity")
        }
        return try await h()
    }

    var syncListDevicesHandler: (() async throws -> [DeviceEntry])?
    func syncListDevices() async throws  -> [DeviceEntry] {
        record("syncListDevices")
        guard let h = syncListDevicesHandler else {
            if let inner { return try await inner.syncListDevices() }
            throw CoreError.Failed(message: "not faked: syncListDevices")
        }
        return try await h()
    }

    var syncListPendingHandler: (() async throws -> [PendingDevice])?
    func syncListPending() async throws  -> [PendingDevice] {
        record("syncListPending")
        guard let h = syncListPendingHandler else {
            if let inner { return try await inner.syncListPending() }
            throw CoreError.Failed(message: "not faked: syncListPending")
        }
        return try await h()
    }

    var syncRemoveDeviceHandler: ((String) async throws -> Void)?
    func syncRemoveDevice(deviceId: String) async throws {
        record("syncRemoveDevice")
        guard let h = syncRemoveDeviceHandler else {
            if let inner { return try await inner.syncRemoveDevice(deviceId: deviceId) }
            throw CoreError.Failed(message: "not faked: syncRemoveDevice")
        }
        return try await h(deviceId)
    }

    var syncResetIdentityHandler: (() async throws -> Void)?
    func syncResetIdentity() async throws {
        record("syncResetIdentity")
        guard let h = syncResetIdentityHandler else {
            if let inner { return try await inner.syncResetIdentity() }
            throw CoreError.Failed(message: "not faked: syncResetIdentity")
        }
        return try await h()
    }

    var syncSetDeviceNameHandler: ((String) async throws -> Void)?
    func syncSetDeviceName(name: String) async throws {
        record("syncSetDeviceName")
        guard let h = syncSetDeviceNameHandler else {
            if let inner { return try await inner.syncSetDeviceName(name: name) }
            throw CoreError.Failed(message: "not faked: syncSetDeviceName")
        }
        return try await h(name)
    }

    var syncSetEnabledHandler: ((Bool) async throws -> Void)?
    func syncSetEnabled(enabled: Bool) async throws {
        record("syncSetEnabled")
        guard let h = syncSetEnabledHandler else {
            if let inner { return try await inner.syncSetEnabled(enabled: enabled) }
            throw CoreError.Failed(message: "not faked: syncSetEnabled")
        }
        return try await h(enabled)
    }

    var syncThisDeviceHandler: (() async throws -> ThisDevice?)?
    func syncThisDevice() async throws  -> ThisDevice? {
        record("syncThisDevice")
        guard let h = syncThisDeviceHandler else {
            if let inner { return try await inner.syncThisDevice() }
            throw CoreError.Failed(message: "not faked: syncThisDevice")
        }
        return try await h()
    }

    var syncWakeHandler: (() async throws -> Void)?
    func syncWake() async throws {
        record("syncWake")
        guard let h = syncWakeHandler else {
            if let inner { return try await inner.syncWake() }
            throw CoreError.Failed(message: "not faked: syncWake")
        }
        return try await h()
    }

    var syncWebUiUrlHandler: (() async throws -> String?)?
    func syncWebUiUrl() async throws  -> String? {
        record("syncWebUiUrl")
        guard let h = syncWebUiUrlHandler else {
            if let inner { return try await inner.syncWebUiUrl() }
            throw CoreError.Failed(message: "not faked: syncWebUiUrl")
        }
        return try await h()
    }

    var translateChapterHandler: ((Uuid, Number, String, Bool) async throws -> Void)?
    func translateChapter(bookId: Uuid, chapterId: Number, model: String, useCache: Bool) async throws {
        record("translateChapter")
        guard let h = translateChapterHandler else {
            if let inner { return try await inner.translateChapter(bookId: bookId, chapterId: chapterId, model: model, useCache: useCache) }
            throw CoreError.Failed(message: "not faked: translateChapter")
        }
        return try await h(bookId, chapterId, model, useCache)
    }

    var translateParagraphHandler: ((Uuid, Number, String, Bool) async throws -> Void)?
    func translateParagraph(bookId: Uuid, paragraphId: Number, model: String, useCache: Bool) async throws {
        record("translateParagraph")
        guard let h = translateParagraphHandler else {
            if let inner { return try await inner.translateParagraph(bookId: bookId, paragraphId: paragraphId, model: model, useCache: useCache) }
            throw CoreError.Failed(message: "not faked: translateParagraph")
        }
        return try await h(bookId, paragraphId, model, useCache)
    }

    var updateConfigHandler: ((AppConfig) async throws -> Void)?
    func updateConfig(config: AppConfig) async throws {
        record("updateConfig")
        guard let h = updateConfigHandler else {
            if let inner { return try await inner.updateConfig(config: config) }
            throw CoreError.Failed(message: "not faked: updateConfig")
        }
        return try await h(config)
    }

}
