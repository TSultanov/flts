package com.TS.FLTS.stores

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.TS.FLTS.core.CoreService
import com.TS.FLTS.core.EventHub
import com.TS.FLTS.core.userMessage
import java.text.Collator
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.flts_ffi.BookMeta
import uniffi.flts_ffi.CoreEvent
import uniffi.flts_ffi.EpubPreview
import uniffi.flts_ffi.ImportProgress

/// A folder in the library tree, built from each book's `path`.
data class LibraryFolder(
    val path: List<String>,
    val folders: List<LibraryFolder> = emptyList(),
    val books: List<BookMeta> = emptyList(),
) {
    val id: String get() = "folder:" + path.joinToString("/")
    val name: String get() = path.lastOrNull() ?: "Library"

    /// Every folder below this one, at any depth.
    val allFolders: List<LibraryFolder> get() = folders.flatMap { listOf(it) + it.allFolders }

    /// Every folder path, depth first, for move pickers.
    val allPaths: List<List<String>> get() = listOf(path) + folders.flatMap { it.allPaths }

    companion object {
        fun build(books: List<BookMeta>): LibraryFolder = node(emptyList(), books)

        private fun node(path: List<String>, books: List<BookMeta>): LibraryFolder {
            val collator = Collator.getInstance().apply { strength = Collator.SECONDARY }
            val depth = path.size
            val (here, deeper) = books.partition { it.path.size <= depth }
            val folders = deeper.groupBy { it.path[depth] }
                .map { (name, inside) -> node(path + name, inside) }
                .sortedWith { a, b -> collator.compare(a.name, b.name) }
            return LibraryFolder(path, folders, here.sortedWith { a, b -> collator.compare(a.title, b.title) })
        }
    }
}

class LibraryStore(private val events: EventHub, private val scope: CoroutineScope) : CoreBound {
    var books by mutableStateOf<List<BookMeta>>(emptyList())
        private set
    var tree by mutableStateOf(LibraryFolder(emptyList()))
        private set
    var loaded by mutableStateOf(false)
        private set
    var importProgress by mutableStateOf<ImportProgress?>(null)
        private set
    var error by mutableStateOf<String?>(null)

    /// Open folders, kept here so they survive a visit to a book.
    var expandedFolders by mutableStateOf<Set<List<String>>>(emptySet())

    var core: CoreService? = null
        private set

    override fun attach(core: CoreService) {
        this.core = core
        events.on { event ->
            when (event) {
                is CoreEvent.LibraryUpdated -> {
                    importProgress = null
                    scheduleReload()
                }
                // Emitted per saved paragraph during a chapter translation.
                is CoreEvent.BookUpdated -> scheduleReload()
                is CoreEvent.LibraryImportProgress ->
                    importProgress = event.progress.takeIf { it.done < it.total }
                else -> {}
            }
        }
        scope.launch { reload() }
    }

    private var reloadScheduled = false

    /// One `list_books` per burst of events.
    private fun scheduleReload() {
        if (reloadScheduled) return
        reloadScheduled = true
        scope.launch {
            delay(300)
            reloadScheduled = false
            reload()
        }
    }

    suspend fun reload() {
        val core = core ?: return
        try {
            val listed = core.listBooks()
            books = listed
            tree = LibraryFolder.build(listed)
            loaded = true
        } catch (e: Exception) {
            error = e.userMessage
        }
    }

    fun book(id: UUID?): BookMeta? = id?.let { books.firstOrNull { book -> book.id == it } }

    suspend fun delete(ids: List<UUID>) {
        val core = core ?: return
        for (id in ids) {
            try {
                core.deleteBook(id)
            } catch (e: Exception) {
                error = e.userMessage
            }
        }
        reload()
    }

    suspend fun move(ids: List<UUID>, path: List<String>) {
        val core = core ?: return
        for (id in ids) {
            try {
                core.moveBook(id, path)
            } catch (e: Exception) {
                error = e.userMessage
            }
        }
        reload()
    }

    // Import

    suspend fun parseEpub(path: String): EpubPreview = ready().parseEpub(path)

    suspend fun importEpub(preview: EpubPreview, sourceLanguageId: String, chapters: List<Int>? = null): UUID {
        val all = preview.chapters().indices.toList()
        val id = ready().importEpub(preview, sourceLanguageId, chapters ?: all)
        reload()
        return id
    }

    suspend fun importText(title: String, text: String, sourceLanguageId: String): UUID {
        val id = ready().importPlainText(title, text, sourceLanguageId)
        reload()
        return id
    }

    private fun ready(): CoreService = core ?: throw IllegalStateException("Not ready")
}
