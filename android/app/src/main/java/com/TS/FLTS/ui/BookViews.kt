package com.TS.FLTS.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.FormatListBulleted
import androidx.compose.material.icons.outlined.FormatSize
import androidx.compose.material.icons.outlined.Translate
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.TS.FLTS.core.AppPrefs
import com.TS.FLTS.reader.ReaderControls
import com.TS.FLTS.reader.ReaderHost
import com.TS.FLTS.stores.AppModel
import com.TS.FLTS.stores.BookSession
import java.util.UUID
import kotlin.math.roundToInt
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import uniffi.flts_ffi.ChapterMeta

/// An open book: the reader, with the chapter list sliding over it from the
/// left (hidden by default) and the word panel along the bottom.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BookPage(model: AppModel, bookId: UUID) {
    val core = model.core ?: return
    val session = remember(bookId) { BookSession(bookId, core, model.events, model.scope) }
    val controls = remember(bookId) { ReaderControls() }
    var chaptersShown by remember { mutableStateOf(false) }
    var wordExpanded by remember { mutableStateOf(false) }

    LaunchedEffect(session) { session.start() }
    DisposableEffect(session) { onDispose { session.close() } }

    val book = session.book
    // A one-chapter book has no chapter list at all, as in the web app.
    val singleChapter = book.loaded && book.chapters.size <= 1
    val toggleChapters = { if (!singleChapter) chaptersShown = !chaptersShown }
    controls.hideChapters = { chaptersShown = false }
    controls.toggleChapters = toggleChapters
    // With no word chosen there are no details to show.
    controls.toggleWordPanel = { if (wordExpanded || session.word.info != null) wordExpanded = !wordExpanded }
    controls.collapseWordPanel = {
        val was = wordExpanded
        wordExpanded = false
        was
    }
    LaunchedEffect(chaptersShown) {
        controls.chaptersShown = chaptersShown
        if (!chaptersShown) controls.focus()
    }
    // The panel folds away with its word.
    LaunchedEffect(session.word.selection) { if (session.word.selection == null) wordExpanded = false }

    // Back goes one step: the chapter list, the word's details, then the library.
    BackHandler {
        when {
            chaptersShown -> chaptersShown = false
            wordExpanded -> wordExpanded = false
            else -> model.selectedBookId = null
        }
    }

    val chapter = session.chapter
    val translateChapter = {
        val id = book.currentChapter
        if (id != null && book.canTranslate(id)) {
            val modelName = model.config.config?.model ?: ""
            model.scope.launch { book.translateChapter(id, modelName) }
        }
    }
    val chapterTitle = if (book.chapters.size > 1) book.chapters.firstOrNull { it.id == book.currentChapter }?.title.orEmpty() else ""

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    // The book names the page; the chapter is under it when there is one to name.
                    Column {
                        Text(
                            model.library.book(bookId)?.title.orEmpty(), maxLines = 1, overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.titleMedium)
                        if (chapterTitle.isNotEmpty()) {
                            Text(
                                chapterTitle, maxLines = 1, overflow = TextOverflow.Ellipsis,
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                },
                navigationIcon = {
                    Row {
                        IconButton(onClick = { model.selectedBookId = null }) {
                            Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = "Library")
                        }
                        if (!singleChapter) {
                            IconButton(onClick = toggleChapters) {
                                Icon(Icons.AutoMirrored.Outlined.FormatListBulleted, contentDescription = "Chapters")
                            }
                        }
                    }
                },
                actions = {
                    if (chapter != null && chapter.loaded && chapter.untranslatedCount > 0) {
                        IconButton(onClick = { translateChapter() }, enabled = book.canTranslate(chapter.chapterId)) {
                            Icon(Icons.Outlined.Translate, contentDescription = "Translate Chapter")
                        }
                    }
                    ReadingMenu()
                },
            )
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            when {
                chapter != null && chapter.loaded -> {
                    key(chapter.chapterId) {
                        ReaderHost(
                            book = book, chapter = chapter, word = session.word, activity = model.activity,
                            controls = controls, model = model.config.config?.model ?: "", scope = model.scope,
                            modifier = Modifier.fillMaxSize().padding(bottom = WordPanelBarHeight))
                    }
                    WordPanel(model, session, wordExpanded, { wordExpanded = it }, Modifier.align(Alignment.BottomCenter))
                }
                chapter != null || !book.loaded ->
                    CircularProgressIndicator(Modifier.align(Alignment.Center))
                book.chapters.isEmpty() -> CenteredMessage(progress = false, title = "This Book Could Not Be Opened")
                else -> CenteredMessage(progress = false, title = "Select a Chapter")
            }

            val failure = model.activity.lastFailure
            val chapterError = chapter?.error
            val otherError = book.error ?: session.word.error
            when {
                chapterError != null -> ErrorBanner(chapterError) { chapter.error = null }
                failure != null -> ErrorBanner("Translation failed: $failure") { model.activity.clearFailure() }
                otherError != null -> ErrorBanner(otherError) {
                    book.error = null
                    session.word.error = null
                }
            }

            // Over the page, not beside it: the text keeps its layout.
            AnimatedVisibility(
                visible = chaptersShown,
                enter = slideInHorizontally(tween(if (AppPrefs.animationsOff) 0 else 180)) { -it },
                exit = slideOutHorizontally(tween(if (AppPrefs.animationsOff) 0 else 180)) { -it },
            ) {
                ChaptersPanel(session, model.scope, close = { chaptersShown = false })
            }
        }
    }
}

/// The chapter list over the reader's left edge; a tap beside it closes it.
@Composable
private fun ChaptersPanel(session: BookSession, scope: CoroutineScope, close: () -> Unit) {
    Row(Modifier.fillMaxSize()) {
        Surface(
            modifier = Modifier.width(280.dp).fillMaxHeight().shadow(8.dp),
            color = MaterialTheme.colorScheme.surfaceContainer,
        ) {
            ChaptersList(session, scope, close)
        }
        Box(
            Modifier.fillMaxSize().clickable(
                interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = close))
    }
}

@Composable
private fun ChaptersList(session: BookSession, scope: CoroutineScope, close: () -> Unit) {
    val book = session.book
    val listState = rememberLazyListState()
    // A long book opens its list at the chapter being read.
    LaunchedEffect(Unit) {
        val index = book.chapters.indexOfFirst { it.id == book.currentChapter }
        if (index > 2) listState.scrollToItem(index - 2)
    }
    Box(Modifier.fillMaxSize()) {
        LazyColumn(state = listState, modifier = Modifier.fillMaxSize()) {
            items(book.chapters, key = { it.id }) { chapter ->
                ChapterRow(
                    chapter = chapter,
                    isCurrent = chapter.id == book.currentChapter,
                    summaryReady = book.summary?.generated?.getOrNull(chapter.id),
                    summarizing = book.summary?.activelyGenerating == chapter.id,
                    onClick = {
                        // As in the web app, picking a chapter closes the list
                        // (also when it is the open one).
                        scope.launch { session.open(chapter.id) }
                        close()
                    },
                )
            }
        }
        when {
            !book.loaded -> CircularProgressIndicator(Modifier.align(Alignment.Center))
            book.chapters.isEmpty() ->
                Text("No chapters", Modifier.align(Alignment.Center), color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun ChapterRow(chapter: ChapterMeta, isCurrent: Boolean, summaryReady: Boolean?, summarizing: Boolean, onClick: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            chapter.title.ifEmpty { "<no title>" },
            modifier = Modifier.weight(1f),
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            fontWeight = if (isCurrent) FontWeight.SemiBold else FontWeight.Normal,
            color = when {
                isCurrent -> MaterialTheme.colorScheme.primary
                summaryReady == false -> MaterialTheme.colorScheme.onSurfaceVariant
                else -> MaterialTheme.colorScheme.onSurface
            },
        )
        if (summarizing) {
            Spacer(Modifier.width(8.dp))
            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
        }
        // As in the web reader: only unfinished chapters show a share.
        if (chapter.translationRatio < 1) {
            Spacer(Modifier.width(8.dp))
            Text(
                "${(chapter.translationRatio * 100).roundToInt()}%", style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
fun ErrorBanner(message: String, dismiss: () -> Unit) {
    Surface(
        modifier = Modifier.fillMaxWidth().padding(12.dp),
        shape = MaterialTheme.shapes.medium,
        color = MaterialTheme.colorScheme.errorContainer,
        contentColor = MaterialTheme.colorScheme.onErrorContainer,
        shadowElevation = 4.dp,
    ) {
        Row(Modifier.padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Outlined.WarningAmber, contentDescription = null)
            Spacer(Modifier.width(8.dp))
            SelectionContainer(Modifier.weight(1f)) {
                Text(message, maxLines = 3, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium)
            }
            TextButton(onClick = dismiss) { Text("Dismiss") }
        }
    }
}

/// Text size and appearance from the reader.
@Composable
private fun ReadingMenu() {
    var shown by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { shown = true }) {
            Icon(Icons.Outlined.FormatSize, contentDescription = "Text Size and Appearance")
        }
        // Stays open while the size is stepped.
        DropdownMenu(expanded = shown, onDismissRequest = { shown = false }) {
            DropdownMenuItem(text = { Text("Smaller Text") }, onClick = { ReaderPreferences.fontSize -= 1 })
            DropdownMenuItem(text = { Text("Larger Text") }, onClick = { ReaderPreferences.fontSize += 1 })
            DropdownMenuItem(
                text = { Text("Default Size") },
                onClick = { ReaderPreferences.fontSize = ReaderPreferences.DEFAULT_FONT_SIZE })
            HorizontalDivider()
            DropdownMenuItem(
                text = { Text("Narrower Margins") },
                onClick = { ReaderPreferences.marginScale -= ReaderPreferences.MARGIN_STEP })
            DropdownMenuItem(
                text = { Text("Wider Margins") },
                onClick = { ReaderPreferences.marginScale += ReaderPreferences.MARGIN_STEP })
            DropdownMenuItem(text = { Text("Default Margins") }, onClick = { ReaderPreferences.marginScale = 1f })
            HorizontalDivider()
            for (appearance in Appearance.entries) {
                DropdownMenuItem(
                    text = { Text(appearance.title) },
                    leadingIcon = { RadioButton(selected = ReaderPreferences.appearance == appearance, onClick = null) },
                    onClick = { ReaderPreferences.appearance = appearance },
                )
            }
        }
    }
}
