package com.TS.FLTS.ui

import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.KeyboardArrowDown
import androidx.compose.material.icons.outlined.KeyboardArrowUp
import androidx.compose.material.icons.outlined.LibraryAdd
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.TS.FLTS.core.AppPrefs
import com.TS.FLTS.stores.AppModel
import com.TS.FLTS.stores.BookSession
import com.TS.FLTS.stores.WordStore
import kotlinx.coroutines.launch
import uniffi.flts_ffi.TranslationProvider
import uniffi.flts_ffi.WordInfo

/// A finger-sized bar; the reader leaves this much room under the page.
val WordPanelBarHeight = 48.dp

/// The selected word at the bottom of the reader: a one-line bar, or (as in
/// the web app) its details in place of the bar. The details grow upward over
/// the page, never resizing it, so pages do not reflow.
@Composable
fun WordPanel(model: AppModel, session: BookSession, expanded: Boolean, setExpanded: (Boolean) -> Unit, modifier: Modifier = Modifier) {
    val info = session.word.info
    // Upper bound the user drags; the panel is shorter when its content is.
    var height by remember { mutableFloatStateOf(AppPrefs.store.getFloat("wordPanelHeight", 320f)) }
    BoxWithConstraints(modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        val limit = maxHeight * 0.75f
        Surface(
            modifier = Modifier.fillMaxWidth(),
            color = MaterialTheme.colorScheme.surfaceContainer,
            shadowElevation = if (expanded) 8.dp else 0.dp,
        ) {
            Column(if (AppPrefs.animationsOff) Modifier else Modifier.animateContentSize()) {
                HorizontalDivider()
                if (expanded && info != null) {
                    val density = LocalDensity.current
                    ResizeHandle { delta ->
                        height = (height - delta / density.density).coerceIn(120f, limit.value)
                        AppPrefs.store.edit().putFloat("wordPanelHeight", height).apply()
                    }
                    WordLine(model, session, info, bar = false, expanded = true, setExpanded = setExpanded)
                    HorizontalDivider()
                    Box(Modifier.heightIn(max = minOf(maxOf(height, 120f).dp, limit)).verticalScroll(rememberScrollState())) {
                        WordDetails(info, wide = this@BoxWithConstraints.maxWidth >= 600.dp)
                    }
                    WordFooter(model, session, info, wide = this@BoxWithConstraints.maxWidth >= 600.dp)
                } else {
                    WordLine(model, session, info, bar = true, expanded = false, setExpanded = setExpanded)
                }
            }
        }
    }
}

@Composable
private fun ResizeHandle(onDrag: (Float) -> Unit) {
    Box(
        Modifier.fillMaxWidth().height(24.dp).pointerInput(Unit) {
            detectVerticalDragGestures { change, amount ->
                change.consume()
                onDrag(amount)
            }
        },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(width = 36.dp, height = 4.dp).background(MaterialTheme.colorScheme.outlineVariant, CircleShape))
    }
}

/// The word with its actions: the collapsed bar (with translations and note),
/// or the heading of the details.
@Composable
private fun WordLine(
    model: AppModel, session: BookSession, info: WordInfo?, bar: Boolean, expanded: Boolean, setExpanded: (Boolean) -> Unit,
) {
    val word = session.word
    val secondary = MaterialTheme.colorScheme.onSurfaceVariant
    Row(
        modifier = Modifier.fillMaxWidth().height(WordPanelBarHeight)
            // The whole bar opens the details, not only its chevron.
            .then(if (bar && info != null) Modifier.clickable { setExpanded(true) } else Modifier)
            .padding(start = 16.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        val chapter = session.chapter
        when {
            info != null -> {
                // The word and its gloss take the room; the actions stay at the trailing edge.
                Row(
                    Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                Text(
                    info.original, fontFamily = FontFamily.Serif, fontWeight = FontWeight.SemiBold, maxLines = 1,
                    style = if (bar) MaterialTheme.typography.titleMedium else MaterialTheme.typography.titleLarge)
                val initial = info.grammar.originalInitialForm
                if (initial.isNotEmpty() && initial != info.original.lowercase()) {
                    Text("($initial)", color = secondary, maxLines = 1)
                }
                if (bar && info.contextualTranslations.isNotEmpty()) {
                    Text("—", color = MaterialTheme.colorScheme.outline)
                    Text(
                        info.contextualTranslations.joinToString(", "), maxLines = 1, overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f, fill = false))
                }
                if (bar && info.note.isNotEmpty()) {
                    Text(
                        info.note, color = secondary, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f, fill = false))
                }
                }
                (word.cardState as? WordStore.CardState.Failed)?.let {
                    // Inline, as in the web app, not only in a tooltip.
                    Text(
                        it.message, color = MaterialTheme.colorScheme.error, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.labelSmall, modifier = Modifier.widthIn(max = 160.dp))
                }
                if (info.canCreateCard) {
                    CardButton(word.cardState) { model.scope.launch { word.createCard() } }
                }
            }
            word.selection != null && word.loading -> {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                Spacer(Modifier.weight(1f))
            }
            word.selection != null -> {
                Text(word.surface.orEmpty(), fontFamily = FontFamily.Serif, style = MaterialTheme.typography.titleMedium)
                Text(
                    "This paragraph is not translated yet.", color = secondary, maxLines = 1,
                    overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            }
            chapter != null && chapter.loaded && chapter.untranslatedCount > 0 && !session.book.canTranslate(chapter.chapterId) -> {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                Text(
                    "Preparing this chapter for translation…", color = secondary, maxLines = 1,
                    overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            }
            else -> Text(
                "Select a word to show its translation", color = secondary, maxLines = 1,
                overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        }
        IconButton(onClick = { setExpanded(!expanded) }, enabled = expanded || info != null) {
            Icon(
                if (expanded) Icons.Outlined.KeyboardArrowDown else Icons.Outlined.KeyboardArrowUp,
                contentDescription = if (expanded) "Hide word details" else "Show word details")
        }
    }
}

@Composable
private fun CardButton(state: WordStore.CardState, action: () -> Unit) {
    when (state) {
        WordStore.CardState.Idle, is WordStore.CardState.Failed ->
            IconButton(onClick = action) {
                Icon(
                    Icons.Outlined.LibraryAdd, contentDescription = "Add to Anki",
                    tint = if (state is WordStore.CardState.Failed) MaterialTheme.colorScheme.error
                    else MaterialTheme.colorScheme.onSurface)
            }
        WordStore.CardState.Creating -> CircularProgressIndicator(Modifier.padding(12.dp).size(18.dp), strokeWidth = 2.dp)
        WordStore.CardState.Created ->
            Icon(
                Icons.Outlined.CheckCircle, contentDescription = "Card added",
                tint = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(12.dp))
        WordStore.CardState.Exists -> {}
    }
}

/// Model and actions for the shown word, under the details.
@Composable
private fun WordFooter(model: AppModel, session: BookSession, info: WordInfo, wide: Boolean) {
    // Defaults to the model that made the current translation, as in the web app.
    var retranslateModel by remember { mutableStateOf(info.translationModel) }
    LaunchedEffect(info.translationModel) { retranslateModel = info.translationModel }
    var picking by remember { mutableStateOf(false) }
    HorizontalDivider()
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (wide) {
            Text(
                "Translate paragraph again with", color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium)
            Spacer(Modifier.width(8.dp))
        }
        val models = model.config.models(model.config.config?.translationProvider ?: TranslationProvider.GOOGLE)
        Box(Modifier.weight(1f, fill = false)) {
            TextButton(onClick = { picking = true }) {
                Text(
                    models.firstOrNull { it.id == retranslateModel }?.name ?: retranslateModel, maxLines = 1,
                    overflow = TextOverflow.Ellipsis)
            }
            DropdownMenu(expanded = picking, onDismissRequest = { picking = false }) {
                for (option in models) {
                    DropdownMenuItem(text = { Text(option.name) }, onClick = {
                        retranslateModel = option.id
                        picking = false
                    })
                }
            }
        }
        Spacer(Modifier.weight(1f))
        val ref = session.word.selection
        if (ref != null) {
            val activity = model.activity.activity(ref.bookId, ref.paragraphId)
            if (activity != null) {
                CircularProgressIndicator(
                    progress = { activity.progressChars.toFloat() / maxOf(activity.expectedChars, 1) },
                    modifier = Modifier.size(18.dp), strokeWidth = 2.dp)
                Spacer(Modifier.width(8.dp))
            }
            TextButton(
                enabled = activity == null,
                onClick = {
                    val chosen = retranslateModel
                    model.scope.launch { session.chapter?.translate(ref.paragraphId, chosen, useCache = false) }
                },
            ) { Text("Translate Again") }
        }
    }
}

@Composable
private fun WordDetails(info: WordInfo, wide: Boolean) {
    SelectionContainer {
        // Side by side where there is room; stacked on a phone.
        if (wide) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp), horizontalArrangement = Arrangement.spacedBy(40.dp)) {
                Column(Modifier.weight(1f).widthIn(max = 560.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) { Meaning(info) }
                Column(Modifier.widthIn(min = 220.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) { Reference(info) }
            }
        } else {
            Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                Meaning(info)
                Reference(info)
            }
        }
    }
}

@Composable
private fun Meaning(info: WordInfo) {
    DetailSection("Translations") {
        Text(info.contextualTranslations.joinToString(", "), style = MaterialTheme.typography.titleMedium)
    }
    if (info.note.isNotEmpty()) DetailSection("Note") { Text(info.note) }
    DetailSection("Sentence") { Text(info.fullSentenceTranslation) }
}

@Composable
private fun Reference(info: WordInfo) {
    val rows = listOf(
        "Part of speech" to info.grammar.partOfSpeech,
        "Initial form" to info.grammar.originalInitialForm,
        "Number" to info.grammar.plurality,
        "Person" to info.grammar.person,
        "Tense" to info.grammar.tense,
        "Case" to info.grammar.grammaticalCase,
        "Other" to info.grammar.other,
    ).filter { !it.second.isNullOrEmpty() }
    if (rows.isEmpty()) return
    DetailSection("Grammar") {
        for ((label, value) in rows) {
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.width(110.dp))
                // The model's tags come as "common_noun".
                Text(value.orEmpty().replace('_', ' '))
            }
        }
    }
}

@Composable
private fun DetailSection(title: String, content: @Composable () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(
            title.uppercase(), style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold,
            color = MaterialTheme.colorScheme.onSurfaceVariant)
        content()
    }
}
