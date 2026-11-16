package com.TS.FLTS.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.LibraryBooks
import androidx.compose.material.icons.automirrored.outlined.MenuBook
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Cancel
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.ChevronRight
import androidx.compose.material.icons.outlined.CreateNewFolder
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.ExpandMore
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.TS.FLTS.core.AppPrefs
import com.TS.FLTS.stores.AppModel
import com.TS.FLTS.stores.LibraryFolder
import java.text.Collator
import java.util.UUID
import kotlin.math.roundToInt
import kotlinx.coroutines.launch
import uniffi.flts_ffi.BookMeta
import uniffi.flts_ffi.ImportProgress

/// The library as its own page: folders and books; opening a book leaves it.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LibraryPage(model: AppModel) {
    val library = model.library
    var editing by remember { mutableStateOf(false) }
    var selection by remember { mutableStateOf<Set<UUID>>(emptySet()) }
    var moving by remember { mutableStateOf<List<BookMeta>>(emptyList()) }
    var deleting by remember { mutableStateOf<List<BookMeta>>(emptyList()) }

    // Selecting is edit mode: even one book needs the bar there, and there is none outside it.
    val selectedBooks = selection.mapNotNull { library.book(it) }
    // Leaving edit mode drops the selection it made.
    val setEditing = { on: Boolean ->
        editing = on
        if (!on) selection = emptySet()
    }
    BackHandler(enabled = editing) { setEditing(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Library") },
                navigationIcon = {
                    // Selecting several books (to move or delete them) needs edit mode.
                    TextButton(onClick = { setEditing(!editing) }, modifier = Modifier.testTag("library.select")) {
                        Text(if (editing) "Done" else "Edit")
                    }
                },
                actions = {
                    AppToolbarActions(model)
                    IconButton(onClick = { model.showImport = true }, modifier = Modifier.testTag("library.import")) {
                        Icon(Icons.Outlined.Add, contentDescription = "Import")
                    }
                },
            )
        },
        bottomBar = {
            if (editing && selectedBooks.isNotEmpty()) {
                SelectionBar(
                    count = selectedBooks.size,
                    move = { moving = selectedBooks },
                    delete = { deleting = selectedBooks },
                    clear = { selection = emptySet() })
            }
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            // Above the list, so they show even when the library becomes empty.
            library.error?.let { ErrorBanner(it) { library.error = null } }
            model.deletedBookNotice?.let { NoticeBar(it) { model.deletedBookNotice = null } }
            if (library.loaded && library.books.isEmpty()) {
                EmptyLibrary { model.showImport = true }
            } else {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
                    LazyColumn(Modifier.widthIn(max = 900.dp).fillMaxSize()) {
                        library.importProgress?.let { progress ->
                            item(key = "import-progress") { ImportProgressRow(progress) }
                        }
                        items(rows(library.tree, library.expandedFolders, 0), key = { it.key }) { row ->
                            when (row) {
                                is LibraryRow.Folder -> FolderRow(
                                    row.folder, row.depth, open = row.folder.path in library.expandedFolders,
                                    toggle = {
                                        val path = row.folder.path
                                        val expanded = library.expandedFolders
                                        library.expandedFolders = if (path in expanded) expanded - setOf(path) else expanded + setOf(path)
                                    })
                                is LibraryRow.Book -> {
                                    val book = row.book
                                    val selected = book.id in selection
                                    BookRow(
                                        book, row.depth, editing = editing, selected = selected,
                                        // The menu acts on the whole selection when the row is part of it.
                                        menuBooks = if (selected && selectedBooks.size > 1) selectedBooks else listOf(book),
                                        tap = {
                                            if (!editing) model.selectedBookId = book.id
                                            else selection = if (selected) selection - book.id else selection + book.id
                                        },
                                        open = { model.selectedBookId = book.id },
                                        move = { moving = it },
                                        delete = { deleting = it })
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if (moving.isNotEmpty()) {
        MoveBookSheet(model, moving, dismiss = { moving = emptyList() })
    }
    if (deleting.isNotEmpty()) {
        val books = deleting
        AlertDialog(
            onDismissRequest = { deleting = emptyList() },
            title = { Text(if (books.size == 1) "Delete “${books[0].title}”?" else "Delete ${books.size} books?") },
            text = { Text("The book and its translations are removed from every synced device.") },
            confirmButton = {
                TextButton(onClick = {
                    val ids = books.map { it.id }
                    model.scope.launch { library.delete(ids) }
                    selection = selection - ids.toSet()
                    deleting = emptyList()
                }) { Text("Delete", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { deleting = emptyList() }) { Text("Cancel") } },
        )
    }
}

private sealed interface LibraryRow {
    val key: String
    val depth: Int

    data class Folder(val folder: LibraryFolder, override val depth: Int) : LibraryRow {
        // Not `folder.id`: a name may hold the "/" it joins with.
        override val key get() = "folder:" + folder.path.joinToString("\u0000")
    }

    data class Book(val book: BookMeta, override val depth: Int) : LibraryRow {
        override val key get() = "book:" + book.id
    }
}

/// The visible rows: a closed folder hides everything under it.
private fun rows(folder: LibraryFolder, expanded: Set<List<String>>, depth: Int): List<LibraryRow> =
    folder.folders.flatMap { child ->
        listOf(LibraryRow.Folder(child, depth)) +
            if (child.path in expanded) rows(child, expanded, depth + 1) else emptyList()
    } + folder.books.map { LibraryRow.Book(it, depth) }

// A full level in, so a folder's books do not read as top-level ones.
private fun levelIndent(depth: Int) = (16 + depth * 20).dp

@Composable
private fun ImportProgressRow(progress: ImportProgress) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text("Importing library…", style = MaterialTheme.typography.bodyMedium)
        LinearProgressIndicator(
            progress = { progress.done.toFloat() / maxOf(progress.total, 1) }, modifier = Modifier.fillMaxWidth())
        Text(
            "${progress.done} of ${progress.total}", style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun EmptyLibrary(import: () -> Unit) {
    Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(
                Icons.AutoMirrored.Outlined.LibraryBooks, contentDescription = null, modifier = Modifier.size(48.dp),
                tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Text("Your Library Is Empty", style = MaterialTheme.typography.titleLarge, textAlign = TextAlign.Center)
            Text(
                "Import an EPUB or paste a text to start reading.", style = MaterialTheme.typography.bodyMedium,
                textAlign = TextAlign.Center, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Button(onClick = import, modifier = Modifier.testTag("library.import.empty")) { Text("Import Book…") }
        }
    }
}

/// A dismissible line above the library list.
@Composable
private fun NoticeBar(text: String, dismiss: () -> Unit) {
    Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
        Column {
            Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Outlined.Delete, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
                Spacer(Modifier.width(8.dp))
                Text(
                    text, modifier = Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                TextButton(onClick = dismiss) { Text("Dismiss") }
            }
            HorizontalDivider()
        }
    }
}

@Composable
private fun FolderRow(folder: LibraryFolder, depth: Int, open: Boolean, toggle: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = toggle).testTag("library.folder." + folder.name)
            .heightIn(min = 48.dp).padding(start = levelIndent(depth), end = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            Icons.Outlined.Folder, contentDescription = null, modifier = Modifier.width(28.dp),
            tint = MaterialTheme.colorScheme.primary)
        Spacer(Modifier.width(12.dp))
        Text(
            folder.name, modifier = Modifier.weight(1f), style = MaterialTheme.typography.titleMedium,
            maxLines = 1, overflow = TextOverflow.Ellipsis)
        Icon(
            if (open) Icons.Outlined.ExpandMore else Icons.Outlined.ChevronRight,
            contentDescription = if (open) "Collapse" else "Expand", tint = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun BookRow(
    book: BookMeta, depth: Int, editing: Boolean, selected: Boolean, menuBooks: List<BookMeta>,
    tap: () -> Unit, open: () -> Unit, move: (List<BookMeta>) -> Unit, delete: (List<BookMeta>) -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier.fillMaxWidth()
                .combinedClickable(onClick = tap, onLongClick = { menu = true })
                .semantics { this.selected = selected }
                .testTag("library.book." + book.title)
                .padding(start = levelIndent(depth), end = 16.dp, top = 8.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (editing) {
                Checkbox(checked = selected, onCheckedChange = null)
                Spacer(Modifier.width(12.dp))
            }
            Icon(
                Icons.AutoMirrored.Outlined.MenuBook, contentDescription = null, modifier = Modifier.width(28.dp),
                tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(
                    book.title, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Medium,
                    maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(counts(book), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            val one = menuBooks.size == 1
            if (one) {
                DropdownMenuItem(text = { Text("Open") }, onClick = { menu = false; open() })
                HorizontalDivider()
            }
            DropdownMenuItem(
                text = { Text(if (one) "Move to Folder…" else "Move ${menuBooks.size} Books to Folder…") },
                onClick = { menu = false; move(menuBooks) })
            DropdownMenuItem(
                text = {
                    Text(if (one) "Delete…" else "Delete ${menuBooks.size} Books…", color = MaterialTheme.colorScheme.error)
                },
                onClick = { menu = false; delete(menuBooks) })
        }
    }
}

private fun counts(book: BookMeta): String {
    val chapters = if (book.chaptersCount == 1) "1 chapter" else "${book.chaptersCount} chapters"
    // As in the web app: a finished book shows no share.
    if (book.translationRatio >= 1) return chapters
    return "$chapters · ${(book.translationRatio * 100).roundToInt()}% translated"
}

/// Shown while books are selected, so batch actions are visible.
@Composable
private fun SelectionBar(count: Int, move: () -> Unit, delete: () -> Unit, clear: () -> Unit) {
    Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
        Column(Modifier.navigationBarsPadding()) {
            HorizontalDivider()
            Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    "$count selected", modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                TextButton(onClick = move, modifier = Modifier.testTag("library.move")) { Text("Move…") }
                TextButton(onClick = delete, modifier = Modifier.testTag("library.delete")) {
                    Text("Delete…", color = MaterialTheme.colorScheme.error)
                }
                TextButton(onClick = clear) { Text("Clear Selection", maxLines = 1) }
            }
        }
    }
}

/// Picks a destination in the folder tree; new folders exist only here until
/// a book is moved into them (a folder is just its books' path).
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun MoveBookSheet(model: AppModel, books: List<BookMeta>, dismiss: () -> Unit) {
    var target by remember {
        val first = books.firstOrNull()?.path ?: emptyList()
        mutableStateOf(if (books.all { it.path == first }) first else emptyList())
    }
    var created by remember { mutableStateOf<List<List<String>>>(emptyList()) }
    var naming by remember { mutableStateOf(false) }
    var newName by remember { mutableStateOf("") }

    val paths = movePaths(model.library.tree.allPaths.toSet() + created)
    val pendingName = if (naming) newName.trim() else ""
    // A name still being typed counts, so Move does not drop it.
    val destination = if (pendingName.isEmpty()) target else target + pendingName
    val alreadyThere = books.all { it.path == destination }

    val cancelName = {
        naming = false
        newName = ""
    }
    val commitName = {
        if (destination !in paths) created = created + listOf(destination)
        target = destination
        cancelName()
    }

    ModalBottomSheet(onDismissRequest = dismiss) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = dismiss) { Text("Cancel") }
            Text(
                if (books.size == 1) "Move “${books[0].title}”" else "Move ${books.size} books",
                modifier = Modifier.weight(1f), style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center,
                maxLines = 1, overflow = TextOverflow.Ellipsis)
            TextButton(
                onClick = {
                    val ids = books.map { it.id }
                    model.scope.launch { model.library.move(ids, destination) }
                    dismiss()
                },
                enabled = !alreadyThere, modifier = Modifier.testTag("move-confirm"),
            ) { Text("Move") }
        }

        val list = rememberLazyListState()
        LaunchedEffect(target, naming) {
            val index = paths.indexOf(target)
            val shown = list.layoutInfo.visibleItemsInfo
            // The name row hangs under its folder, so it wants the folder near the top.
            val visible = !naming && shown.size > 1 && index > shown.first().index && index < shown.last().index
            if (index < 0 || visible) return@LaunchedEffect
            if (AppPrefs.animationsOff) list.scrollToItem(index) else list.animateScrollToItem(index)
        }
        LazyColumn(
            Modifier.weight(1f, fill = false).fillMaxWidth().padding(6.dp), state = list,
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            items(paths) { path ->
                MoveFolderRow(
                    path, selected = path == target, current = books.all { it.path == path }, select = { target = path })
                if (naming && path == target) {
                    NewFolderRow(target.size + 1, newName, { newName = it }, commit = commitName, cancel = cancelName)
                }
            }
        }
        HorizontalDivider()
        Row(
            Modifier.fillMaxWidth().padding(start = 4.dp, end = 16.dp, top = 4.dp, bottom = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            TextButton(onClick = { naming = true }, enabled = !naming, modifier = Modifier.testTag("move-new-folder")) {
                Icon(Icons.Outlined.CreateNewFolder, contentDescription = null)
                Spacer(Modifier.width(8.dp))
                Text("New Folder")
            }
            Spacer(Modifier.width(8.dp))
            Text(
                "Move to: /" + destination.joinToString("/"),
                modifier = Modifier.weight(1f).testTag("move-destination"), textAlign = TextAlign.End,
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1, overflow = TextOverflow.StartEllipsis)
        }
    }
}

private fun moveIndent(depth: Int) = (depth * 18 + 8).dp

/// Every folder, the new ones included, in the library's order.
private fun movePaths(all: Set<List<String>>): List<List<String>> {
    val collator = Collator.getInstance().apply { strength = Collator.SECONDARY }
    fun walk(parent: List<String>): List<List<String>> {
        val children = all
            .filter { it.size == parent.size + 1 && it.dropLast(1) == parent }
            .sortedWith { a, b -> collator.compare(a.last(), b.last()) }
        return listOf(parent) + children.flatMap(::walk)
    }
    return walk(emptyList())
}

@Composable
private fun MoveFolderRow(path: List<String>, selected: Boolean, current: Boolean, select: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    val content = if (selected) colors.onPrimary else colors.onSurface
    Row(
        Modifier.fillMaxWidth()
            .background(if (selected) colors.primary else Color.Transparent, MaterialTheme.shapes.small)
            .clickable(onClick = select)
            .semantics { this.selected = selected }
            .testTag("move-folder:" + path.joinToString("/"))
            .heightIn(min = 44.dp)
            .padding(start = moveIndent(path.size), end = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            if (path.isEmpty()) Icons.AutoMirrored.Outlined.LibraryBooks else Icons.Outlined.Folder,
            contentDescription = null, modifier = Modifier.width(22.dp), tint = if (selected) colors.onPrimary else colors.primary)
        Spacer(Modifier.width(8.dp))
        Text(path.lastOrNull() ?: "Library", modifier = Modifier.weight(1f), color = content, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (current) {
            Spacer(Modifier.width(8.dp))
            Text("Current", style = MaterialTheme.typography.bodySmall, color = content.copy(alpha = 0.7f))
        }
        if (selected) {
            Spacer(Modifier.width(8.dp))
            Icon(Icons.Outlined.Check, contentDescription = null, modifier = Modifier.size(16.dp), tint = content)
        }
    }
}

@Composable
private fun NewFolderRow(depth: Int, name: String, change: (String) -> Unit, commit: () -> Unit, cancel: () -> Unit) {
    val focus = remember { FocusRequester() }
    Row(
        Modifier.fillMaxWidth().padding(start = moveIndent(depth), end = 10.dp, top = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            Icons.Outlined.CreateNewFolder, contentDescription = null, modifier = Modifier.width(22.dp),
            tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.width(8.dp))
        OutlinedTextField(
            value = name, onValueChange = change, singleLine = true,
            placeholder = { Text("New folder name") },
            // Done adds the folder; it must not also move.
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { commit() }),
            modifier = Modifier.weight(1f).focusRequester(focus).testTag("new-folder-name"))
        IconButton(onClick = cancel) {
            Icon(Icons.Outlined.Cancel, contentDescription = "Cancel New Folder", tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    // Focus requested before the field is attached is lost.
    LaunchedEffect(Unit) { focus.requestFocus() }
}
