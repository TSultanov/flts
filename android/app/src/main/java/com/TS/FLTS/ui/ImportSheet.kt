package com.TS.FLTS.ui

import android.content.Context
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.MenuBook
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material3.Button
import androidx.compose.material3.CenterAlignedTopAppBar
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.TS.FLTS.core.label
import com.TS.FLTS.core.logError
import com.TS.FLTS.core.userMessage
import com.TS.FLTS.platform.FilePicker
import com.TS.FLTS.stores.AppModel
import java.io.File
import kotlinx.coroutines.launch
import uniffi.flts_ffi.EpubChapterSummary
import uniffi.flts_ffi.EpubPreview

private enum class ImportKind(val title: String, val tag: String) {
    EPUB("EPUB", "import.tab.epub"), TEXT("Text", "import.tab.text")
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ImportSheet(model: AppModel, onDismiss: () -> Unit) {
    val context = LocalContext.current.applicationContext
    val close by rememberUpdatedState(onDismiss)
    var kind by remember { mutableStateOf(ImportKind.EPUB) }
    // Both outlive a tab switch, so switching does not discard a parsed book or typed text.
    val epub = remember { EpubImportState(model, context) { close() } }
    val text = remember { TextImportState(model) { close() } }
    DisposableEffect(Unit) {
        onDispose {
            epub.release()
            text.released = true
        }
    }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        DialogSystemBars()
        Scaffold(
            modifier = Modifier.fillMaxSize(),
            topBar = {
                CenterAlignedTopAppBar(
                    navigationIcon = {
                        IconButton(onClick = onDismiss, modifier = Modifier.testTag("import.close")) {
                            Icon(Icons.Filled.Close, contentDescription = "Cancel")
                        }
                    },
                    title = {
                        SingleChoiceSegmentedButtonRow(Modifier.width(220.dp)) {
                            ImportKind.entries.forEachIndexed { index, entry ->
                                SegmentedButton(
                                    selected = kind == entry,
                                    onClick = { kind = entry },
                                    shape = SegmentedButtonDefaults.itemShape(index, ImportKind.entries.size),
                                    modifier = Modifier.testTag(entry.tag),
                                ) { Text(entry.title) }
                            }
                        }
                    },
                )
            },
        ) { padding ->
            Column(Modifier.padding(padding).fillMaxSize()) {
                HorizontalDivider()
                when (kind) {
                    ImportKind.EPUB -> EpubImport(model, epub)
                    ImportKind.TEXT -> TextImport(model, text)
                }
            }
        }
    }
}

private class EpubImportState(
    private val model: AppModel,
    private val context: Context,
    private val close: () -> Unit,
) {
    var preview by mutableStateOf<EpubPreview?>(null)
        private set
    var title by mutableStateOf("")
        private set
    var chapters by mutableStateOf<List<EpubChapterSummary>>(emptyList())
        private set

    /// Chosen chapter indices; empty chapters cannot be chosen.
    var chosen by mutableStateOf<Set<Int>>(emptySet())
    var open by mutableStateOf<Set<Int>>(emptySet())
    var language by mutableStateOf("eng")
    var busy by mutableStateOf(false)
        private set
    var error by mutableStateOf<String?>(null)
        private set

    /// The picked document's copy in the cache; null for a path that is not ours.
    private var copy: File? = null
    private var released = false
    private var importing = false

    val importable: List<Int>
        get() = chapters.indices.filter { chapters[it].paragraphCount > 0 }

    fun pick(uri: Uri) = parse(owned = true) { FilePicker.copyToCache(context, uri) }

    /// A named path is not ours to delete.
    fun load(path: String) = parse(owned = false) { File(path) }

    private fun parse(owned: Boolean, file: suspend () -> File) {
        // Jobs outlive the sheet (as an import must); `released` hands them the cleanup.
        model.scope.launch {
            busy = true
            error = null
            var mine: File? = null
            try {
                val source = file()
                if (owned) mine = source
                val parsed = model.library.parseEpub(source.path)
                if (released) {
                    parsed.close()
                    mine?.delete()
                    return@launch
                }
                val previous = preview
                val previousCopy = copy
                language = model.config.suggestedSourceLanguage(parsed.language())
                title = parsed.title()
                chapters = parsed.chapters()
                chosen = importable.toSet()
                open = emptySet()
                preview = parsed
                copy = mine
                previous?.close()
                previousCopy?.delete()
            } catch (e: Exception) {
                mine?.delete()
                logError("EPUB parse failed: ${e.userMessage}")
                error = "Could not load this file. Please choose a valid EPUB."
            } finally {
                busy = false
            }
        }
    }

    fun importBook() {
        val preview = preview ?: return
        if (importing) return
        importing = true
        model.scope.launch {
            busy = true
            try {
                model.library.importEpub(preview, language, chosen.sorted())
                // As in the web app: back to the library, where the book now is.
                model.selectedBookId = null
                if (!released) close()
            } catch (e: Exception) {
                error = e.userMessage
            } finally {
                busy = false
                importing = false
                if (released) cleanUp()
            }
        }
    }

    fun release() {
        released = true
        // A running import still reads the preview: it cleans up when it ends.
        if (!importing) cleanUp()
    }

    private fun cleanUp() {
        preview?.close()
        preview = null
        copy?.delete()
        copy = null
    }
}

@Composable
private fun EpubImport(model: AppModel, state: EpubImportState) {
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) state.pick(uri)
    }
    val pickFile = {
        val test = FilePicker.uiTestEpub
        if (test != null) state.load(test) else launcher.launch(FilePicker.epubMimeTypes)
    }
    val preview = state.preview

    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (preview != null) {
            val importable = state.importable
            val all = state.chosen.size == importable.size
            Text(state.title, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Select chapters to import", color = MaterialTheme.colorScheme.onSurfaceVariant)
                Spacer(Modifier.weight(1f))
                TextButton(onClick = { state.chosen = if (all) emptySet() else importable.toSet() }) {
                    Text(if (all) "Select None" else "Select All")
                }
            }
            LazyColumn(
                Modifier.weight(1f).fillMaxWidth()
                    .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(8.dp)),
            ) {
                items(state.chapters.size) { index ->
                    ChapterChoice(
                        preview = preview, index = index, chapter = state.chapters[index],
                        isOn = index in state.chosen,
                        onToggle = { on -> state.chosen = if (on) state.chosen + index else state.chosen - index },
                        open = index in state.open,
                        onOpen = { open -> state.open = if (open) state.open + index else state.open - index },
                    )
                }
            }
            LanguagePickerRow(model, "Book language", state.language) { state.language = it }
        } else {
            Column(
                Modifier.weight(1f).fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Icon(
                    Icons.AutoMirrored.Filled.MenuBook, contentDescription = null,
                    modifier = Modifier.size(40.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                FilledTonalButton(onClick = pickFile, modifier = Modifier.testTag("import.pickFile")) {
                    Text("Choose EPUB…")
                }
                if (state.busy) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
            }
        }
        state.error?.let { ErrorText(it) }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            if (preview != null) {
                OutlinedButton(onClick = pickFile, modifier = Modifier.testTag("import.pickFile")) {
                    Text("Choose Another…")
                }
            }
            Spacer(Modifier.weight(1f))
            if (state.busy && preview != null) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
            Button(
                onClick = state::importBook,
                enabled = preview != null && !state.busy && state.chosen.isNotEmpty(),
                modifier = Modifier.testTag("import.submit"),
            ) { Text("Import") }
        }
    }
}

/// One chapter: a checkbox (disabled for an empty chapter) that expands to its text.
@Composable
private fun ChapterChoice(
    preview: EpubPreview,
    index: Int,
    chapter: EpubChapterSummary,
    isOn: Boolean,
    onToggle: (Boolean) -> Unit,
    open: Boolean,
    onOpen: (Boolean) -> Unit,
) {
    Column(Modifier.fillMaxWidth()) {
        Row(
            Modifier.clickable { onOpen(!open) }.padding(end = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Checkbox(checked = isOn, onCheckedChange = onToggle, enabled = chapter.paragraphCount > 0)
            Text(
                chapter.title.ifEmpty { "<no title>" },
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
            )
            Text(
                if (chapter.paragraphCount == 0) "empty" else "${chapter.paragraphCount} ¶",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 8.dp),
            )
            Icon(
                if (open) Icons.Filled.ExpandLess else Icons.Filled.ExpandMore,
                contentDescription = if (open) "Collapse" else "Expand",
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (open) {
            // Keyed on the preview: a replaced one is closed, and calling it throws.
            val paragraphs = remember(preview, index) {
                runCatching { preview.chapterParagraphs(index).take(40) }.getOrDefault(emptyList())
            }
            Column(
                Modifier.padding(start = 48.dp, end = 12.dp, top = 4.dp, bottom = 8.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                for (paragraph in paragraphs) {
                    Text(
                        paragraph, style = MaterialTheme.typography.bodyMedium, fontFamily = FontFamily.Serif,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

private class TextImportState(private val model: AppModel, private val close: () -> Unit) {
    var title by mutableStateOf("")
    var text by mutableStateOf("")
    var language by mutableStateOf("eng")
    var busy by mutableStateOf(false)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    var released = false

    fun importText() {
        model.scope.launch {
            busy = true
            try {
                model.library.importText(title.trim(), text, language)
                model.selectedBookId = null
                if (!released) close()
            } catch (e: Exception) {
                error = e.userMessage
            } finally {
                busy = false
            }
        }
    }
}

@Composable
private fun TextImport(model: AppModel, state: TextImportState) {
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        OutlinedTextField(
            value = state.title, onValueChange = { state.title = it },
            label = { Text("Title") }, singleLine = true,
            modifier = Modifier.fillMaxWidth().testTag("import.title"),
        )
        OutlinedTextField(
            value = state.text, onValueChange = { state.text = it },
            textStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = FontFamily.Serif),
            // A foreign-language text: not for autocorrect.
            keyboardOptions = KeyboardOptions(
                capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false,
            ),
            modifier = Modifier.weight(1f).fillMaxWidth().testTag("import.text"),
        )
        LanguagePickerRow(model, "Text language", state.language) { state.language = it }
        state.error?.let { ErrorText(it) }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Spacer(Modifier.weight(1f))
            Button(
                onClick = state::importText,
                enabled = !state.busy && state.title.isNotBlank() && state.text.isNotBlank(),
                modifier = Modifier.testTag("import.submit"),
            ) { Text("Import") }
        }
    }
}

@Composable
private fun ErrorText(message: String) {
    SelectionContainer {
        Text(message, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
    }
}

@Composable
private fun LanguagePickerRow(model: AppModel, title: String, selection: String, onSelect: (String) -> Unit) {
    var expanded by remember { mutableStateOf(false) }
    val languages = model.config.languages
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(title)
        Spacer(Modifier.weight(1f))
        Box {
            TextButton(onClick = { expanded = true }, modifier = Modifier.testTag("import.language")) {
                Text(languages.firstOrNull { it.id == selection }?.label ?: selection)
                Icon(Icons.Filled.ArrowDropDown, contentDescription = null)
            }
            DropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
                for (language in languages) {
                    DropdownMenuItem(
                        text = { Text(language.label) },
                        onClick = {
                            onSelect(language.id)
                            expanded = false
                        },
                    )
                }
            }
        }
    }
}
