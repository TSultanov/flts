package com.TS.FLTS.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowRight
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.ArrowDropDown
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.FormatSize
import androidx.compose.material.icons.outlined.Layers
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.Remove
import androidx.compose.material.icons.outlined.Storage
import androidx.compose.material.icons.outlined.Sync
import androidx.compose.material.icons.outlined.TextDecrease
import androidx.compose.material.icons.outlined.TextIncrease
import androidx.compose.material.icons.outlined.Translate
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withLink
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.TS.FLTS.core.apiKey
import com.TS.FLTS.core.hasApiKeyForProvider
import com.TS.FLTS.core.label
import com.TS.FLTS.core.userMessage
import com.TS.FLTS.core.withApiKey
import com.TS.FLTS.stores.AppModel
import java.text.Collator
import kotlin.math.roundToInt
import kotlinx.coroutines.launch
import uniffi.flts_ffi.AnkiSyncState
import uniffi.flts_ffi.AppConfig
import uniffi.flts_ffi.ModelInfo
import uniffi.flts_ffi.TranslationProvider

private enum class SettingsPage(val title: String, val icon: ImageVector) {
    TRANSLATION("Translation", Icons.Outlined.Translate),
    READING("Reading", Icons.Outlined.FormatSize),
    SYNC("Sync", Icons.Outlined.Sync),
    ANKI("Anki", Icons.Outlined.Layers),
    STORAGE("Storage", Icons.Outlined.Storage),
}

internal val SettingsWarning = Color(0xFFE08600)
internal val SettingsGood = Color(0xFF2E9E4F)

/// A full-screen sheet with one page per section.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsSheet(model: AppModel, onDismiss: () -> Unit) {
    // The library's sync status opens straight on the Sync page.
    var page by remember {
        mutableStateOf(if (model.settingsPage == SETTINGS_PAGE_SYNC) SettingsPage.SYNC else null)
    }
    LaunchedEffect(Unit) { model.settingsPage = null }
    val back = { if (page != null) page = null else onDismiss() }
    // The dialog is its own window: Back arrives here, not at the activity.
    Dialog(onDismissRequest = back, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        val shown = page
        Scaffold(
            modifier = Modifier.fillMaxSize().testTag("settings"),
            topBar = {
                TopAppBar(
                    title = { Text(shown?.title ?: "Settings") },
                    navigationIcon = {
                        if (shown == null) {
                            IconButton(onClick = onDismiss, modifier = Modifier.testTag("settings.close")) {
                                Icon(Icons.Outlined.Close, contentDescription = "Done")
                            }
                        } else {
                            IconButton(onClick = { page = null }, modifier = Modifier.testTag("settings.back")) {
                                Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = "Settings")
                            }
                        }
                    },
                )
            },
        ) { padding ->
            Box(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding).imePadding()) {
                if (shown == null) {
                    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
                        for (entry in SettingsPage.entries) {
                            ListItem(
                                headlineContent = { Text(entry.title) },
                                leadingContent = { Icon(entry.icon, contentDescription = null) },
                                trailingContent = {
                                    Icon(Icons.AutoMirrored.Outlined.KeyboardArrowRight, contentDescription = null)
                                },
                                modifier = Modifier
                                    .clickable { page = entry }
                                    .testTag("settings.section.${entry.name.lowercase()}"),
                            )
                        }
                    }
                } else {
                    // Keyed: a page's drafts never carry over to another page.
                    key(shown) {
                        Column(
                            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
                            verticalArrangement = Arrangement.spacedBy(20.dp),
                        ) {
                            when (shown) {
                                SettingsPage.TRANSLATION -> TranslationSettings(model)
                                SettingsPage.READING -> ReadingSettings()
                                SettingsPage.SYNC -> SyncSettings(model)
                                SettingsPage.ANKI -> AnkiSettings(model)
                                SettingsPage.STORAGE -> StorageSettings(model)
                            }
                        }
                    }
                }
                // Saves happen on every page; a failure must show wherever it happened.
                model.config.error?.let { error ->
                    Box(Modifier.align(Alignment.BottomCenter)) {
                        ErrorBanner("Could not save: $error") { model.config.error = null }
                    }
                }
            }
        }
    }
}

@Composable
fun OnboardingScreen(model: AppModel) {
    var tried by remember { mutableStateOf(false) }
    val focus = LocalFocusManager.current
    val config = model.config.config
    // Why the app cannot start yet; null once it can (the page then goes away).
    val missing = when {
        config == null -> null
        config.targetLanguageId.isEmpty() -> "Choose the language to translate into."
        !config.hasApiKeyForProvider ->
            "Paste an API key for ${model.config.provider(config.translationProvider)?.name ?: "the provider"}."
        else -> null
    }
    Box(Modifier.fillMaxSize().safeDrawingPadding().testTag("onboarding"), contentAlignment = Alignment.Center) {
        Column(
            Modifier.verticalScroll(rememberScrollState()).padding(24.dp).widthIn(max = 520.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Icon(
                    Icons.Outlined.Translate, contentDescription = null, modifier = Modifier.size(34.dp),
                    tint = MaterialTheme.colorScheme.primary)
                Text("Welcome to FLTS", style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.SemiBold)
            }
            Text(
                "Pick the language you want translations in and a translation provider.",
                color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
            Column(verticalArrangement = Arrangement.spacedBy(20.dp)) {
                TranslationSettings(model, essentialsOnly = true)
            }
            Button(
                onClick = {
                    // Fields save on Done or losing focus; ending editing commits the last one.
                    focus.clearFocus()
                    tried = true
                },
                modifier = Modifier.testTag("onboarding.continue"),
            ) { Text("Continue") }
            if (tried && missing != null) {
                Text(
                    missing, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.testTag("onboarding.missing"))
            }
            Text(
                "You can change these later in Settings.", style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
internal fun SettingsSection(
    title: String? = null, footer: (@Composable () -> Unit)? = null, content: @Composable ColumnScope.() -> Unit,
) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (title != null) {
            Text(
                title, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary,
                modifier = Modifier.padding(horizontal = 4.dp))
        }
        Surface(shape = MaterialTheme.shapes.large, tonalElevation = 1.dp, modifier = Modifier.fillMaxWidth()) {
            Column(
                Modifier.padding(horizontal = 16.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(10.dp),
                content = content)
        }
        if (footer != null) Box(Modifier.padding(horizontal = 4.dp)) { footer() }
    }
}

@Composable
internal fun SettingsFooter(text: String) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

@Composable
internal fun SettingsSwitchRow(
    title: String, checked: Boolean, modifier: Modifier = Modifier, enabled: Boolean = true,
    onChange: (Boolean) -> Unit,
) {
    Row(
        modifier.fillMaxWidth()
            .toggleable(value = checked, enabled = enabled, role = Role.Switch, onValueChange = onChange)
            .padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, Modifier.weight(1f).padding(end = 12.dp))
        Switch(checked = checked, onCheckedChange = null, enabled = enabled)
    }
}

/// Keys, addresses and IDs: never corrected or capitalised.
internal fun settingsLiteralKeyboard(type: KeyboardType = KeyboardType.Ascii, action: ImeAction = ImeAction.Done) =
    KeyboardOptions(
        capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, keyboardType = type, imeAction = action)

/// A text field that writes back on Done or when it loses focus, so the
/// backend is not reconfigured on every keystroke.
@Composable
private fun CommitField(
    title: String, value: String, modifier: Modifier = Modifier, secure: Boolean = false, url: Boolean = false,
    prompt: String? = null, commit: (String) -> Unit,
) {
    var draft by remember { mutableStateOf(value) }
    var focused by remember { mutableStateOf(false) }
    val current by rememberUpdatedState(value)
    val onCommit by rememberUpdatedState(commit)
    val save = { if (draft != current) onCommit(draft) }
    val focus = LocalFocusManager.current
    LaunchedEffect(value) { if (!focused) draft = value }
    // Leaving the page or closing the sheet takes the field away while focused.
    DisposableEffect(Unit) { onDispose { save() } }
    OutlinedTextField(
        value = draft,
        onValueChange = { draft = it },
        label = { Text(title) },
        placeholder = prompt?.let { { Text(it, maxLines = 1, overflow = TextOverflow.Ellipsis) } },
        singleLine = true,
        visualTransformation = if (secure) PasswordVisualTransformation() else VisualTransformation.None,
        keyboardOptions = settingsLiteralKeyboard(
            when {
                secure -> KeyboardType.Password
                url -> KeyboardType.Uri
                else -> KeyboardType.Ascii
            }),
        // Saved by the focus loss.
        keyboardActions = KeyboardActions(onDone = { focus.clearFocus() }),
        modifier = modifier.fillMaxWidth().onFocusChanged {
            if (focused && !it.isFocused) save()
            focused = it.isFocused
        },
    )
}

/// A labelled value that opens a list to choose from. The list is lazy:
/// there are hundreds of languages and models.
@Composable
private fun <T> SettingsPicker(
    title: String, selected: T, options: List<Pair<T, String>>, modifier: Modifier = Modifier, tag: String? = null,
    placeholder: String = "", onPick: (T) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val current = options.firstOrNull { it.first == selected }?.second ?: placeholder
    Row(
        modifier.fillMaxWidth()
            .clickable(role = Role.DropdownList) { open = true }
            .then(if (tag != null) Modifier.testTag(tag) else Modifier)
            .padding(vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, Modifier.padding(end = 16.dp))
        Text(
            current, Modifier.weight(1f), textAlign = TextAlign.End, maxLines = 1, overflow = TextOverflow.Ellipsis,
            color = MaterialTheme.colorScheme.onSurfaceVariant)
        Icon(Icons.Outlined.ArrowDropDown, contentDescription = null)
    }
    if (open) {
        Dialog(onDismissRequest = { open = false }) {
            Surface(shape = MaterialTheme.shapes.extraLarge, tonalElevation = 6.dp) {
                Column(Modifier.padding(vertical = 16.dp)) {
                    Text(
                        title, style = MaterialTheme.typography.titleLarge,
                        modifier = Modifier.padding(start = 24.dp, end = 24.dp, bottom = 8.dp))
                    val start = options.indexOfFirst { it.first == selected }.coerceAtLeast(0)
                    LazyColumn(
                        state = rememberLazyListState(start),
                        modifier = Modifier.heightIn(max = 480.dp).then(
                            if (tag != null) Modifier.testTag("$tag.options") else Modifier),
                    ) {
                        items(options) { (value, label) ->
                            Row(
                                Modifier.fillMaxWidth()
                                    .selectable(selected = value == selected, role = Role.RadioButton) {
                                        open = false
                                        if (value != selected) onPick(value)
                                    }
                                    .then(if (tag != null) Modifier.testTag("$tag.option.$value") else Modifier)
                                    .padding(horizontal = 24.dp, vertical = 12.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                RadioButton(selected = value == selected, onClick = null)
                                Spacer(Modifier.width(16.dp))
                                Text(label)
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Onboarding passes essentialsOnly: only what the app needs to start.
@Composable
private fun TranslationSettings(model: AppModel, essentialsOnly: Boolean = false) {
    val store = model.config
    val config = store.config
    if (config == null) {
        CircularProgressIndicator()
        return
    }
    SettingsSection {
        SettingsPicker(
            title = "Translate into", selected = config.targetLanguageId,
            options = store.languages.map { it.id to it.label }, tag = "settings.targetLanguage",
            placeholder = "Choose…",
        ) { id -> model.scope.launch { store.update { it.copy(targetLanguageId = id) } } }
    }
    SettingsSection {
        val provider = config.translationProvider
        SettingsPicker(
            title = "Provider", selected = provider, options = store.providers.map { it.id to it.name },
            tag = "settings.provider",
        ) { picked ->
            model.scope.launch {
                store.update { it.copy(translationProvider = picked, model = store.provider(picked)?.defaultModel ?: it.model) }
            }
        }
        // A field per provider: a draft typed for one must not be saved under another.
        key(provider) {
            CommitField(
                title = "API key", value = config.apiKey(provider) ?: "", secure = true,
                prompt = "Paste your ${store.provider(provider)?.name ?: "provider"} API key",
                modifier = Modifier.testTag("settings.apiKey"),
            ) { key -> model.scope.launch { store.update { it.withApiKey(key, provider) } } }
        }
        if (!essentialsOnly) ModelPicker(model, config)
    }
    if (!essentialsOnly) {
        SettingsSection("Performance") {
            val n = config.translationConcurrency
            val set = { value: Int -> model.scope.launch { store.update { it.copy(translationConcurrency = value) } } }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Parallel translations: $n", Modifier.weight(1f).testTag("settings.concurrency"))
                IconButton(
                    onClick = { set(n - 1) }, enabled = n > 1, modifier = Modifier.testTag("settings.concurrency.less"),
                ) { Icon(Icons.Outlined.Remove, contentDescription = "Fewer parallel translations") }
                IconButton(
                    onClick = { set(n + 1) }, enabled = n < 16, modifier = Modifier.testTag("settings.concurrency.more"),
                ) { Icon(Icons.Outlined.Add, contentDescription = "More parallel translations") }
            }
        }
        if (config.translationProvider == TranslationProvider.GOOGLE) {
            SettingsSection(footer = {
                SettingsFooter("Gemini keeps prompt caches on Google's servers for a while; this removes them now.")
            }) {
                PurgeCachesRow(model, hasKey = !config.geminiApiKey.isNullOrEmpty())
            }
        }
    }
}

@Composable
private fun PurgeCachesRow(model: AppModel, hasKey: Boolean) {
    var busy by remember { mutableStateOf(false) }
    var result by remember { mutableStateOf<String?>(null) }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        OutlinedButton(
            onClick = {
                busy = true
                model.scope.launch {
                    result = try {
                        val n = model.config.purgeGeminiCaches()
                        if (n == 1) "Removed 1 cached item." else "Removed $n cached items."
                    } catch (e: Exception) {
                        e.userMessage
                    }
                    busy = false
                }
            },
            enabled = !busy && hasKey,
            modifier = Modifier.testTag("settings.purgeCaches"),
        ) { Text(if (busy) "Removing…" else "Remove Server Caches") }
        result?.let { Text(it, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f)) }
    }
}

/// OpenRouter (and providers that ask for it) group models by family: the
/// id prefix before "/".
private object ModelFamilies {
    fun family(id: String): String {
        val normalized = id.removePrefix("~")
        val slash = normalized.indexOf('/')
        return if (slash <= 0) "other" else normalized.substring(0, slash)
    }

    fun label(family: String): String =
        if (family == "other") "Other"
        else family.split("-").filter { it.isNotEmpty() }.joinToString(" ") { it.replaceFirstChar(Char::uppercase) }

    fun families(models: List<ModelInfo>): List<String> {
        val collator = Collator.getInstance()
        return models.map { family(it.id) }.distinct().sortedWith { a, b -> collator.compare(label(a), label(b)) }
    }
}

@Composable
private fun ModelPicker(model: AppModel, config: AppConfig) {
    val store = model.config
    val models = store.models(config.translationProvider)
    val byFamily = config.translationProvider == TranslationProvider.OPENROUTER ||
        store.provider(config.translationProvider)?.modelSelection == "family"
    val orphan = models.isNotEmpty() && models.none { it.id == config.model }
    var family by remember { mutableStateOf("") }
    if (byFamily && models.isNotEmpty()) {
        val families = remember(models) { ModelFamilies.families(models) }
        SettingsPicker(
            title = "Family", selected = if (family in families) family else ModelFamilies.family(config.model),
            options = families.map { it to ModelFamilies.label(it) }, tag = "settings.modelFamily",
        ) { picked ->
            family = picked
            models.firstOrNull { ModelFamilies.family(it.id) == picked }?.let { first ->
                model.scope.launch { store.update { it.copy(model = first.id) } }
            }
        }
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        val shown =
            if (byFamily) models.filter { ModelFamilies.family(it.id) == ModelFamilies.family(config.model) } else models
        val options = (if (orphan) listOf(config.model to config.model) else emptyList()) + shown.map { it.id to it.name }
        SettingsPicker(
            title = "Model", selected = config.model, options = options, modifier = Modifier.weight(1f),
            tag = "settings.model", placeholder = config.model,
        ) { id -> model.scope.launch { store.update { it.copy(model = id) } } }
        if (store.modelsLoading) {
            CircularProgressIndicator(Modifier.padding(12.dp).size(24.dp), strokeWidth = 2.dp)
        } else {
            IconButton(
                onClick = { model.scope.launch { store.loadModels() } },
                modifier = Modifier.testTag("settings.refreshModels"),
            ) { Icon(Icons.Outlined.Refresh, contentDescription = "Refresh the model list") }
        }
    }
    if (orphan) {
        Text("Not in the current catalog", style = MaterialTheme.typography.bodySmall, color = SettingsWarning)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ReadingSettings() {
    SettingsSection {
        SingleChoiceSegmentedButtonRow(
            Modifier.fillMaxWidth().semantics { contentDescription = "Appearance" }.testTag("settings.appearance"),
        ) {
            Appearance.entries.forEachIndexed { index, look ->
                SegmentedButton(
                    selected = ReaderPreferences.appearance == look,
                    onClick = { ReaderPreferences.appearance = look },
                    shape = SegmentedButtonDefaults.itemShape(index, Appearance.entries.size),
                    modifier = Modifier.testTag("settings.appearance.${look.name.lowercase()}"),
                ) { Text(look.title, maxLines = 1) }
            }
        }
    }
    SettingsSection("Text") {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(Icons.Outlined.TextDecrease, contentDescription = null)
            Slider(
                value = ReaderPreferences.fontSize,
                onValueChange = { ReaderPreferences.fontSize = it.roundToInt().toFloat() },
                valueRange = 12f..36f,
                // Whole sizes only.
                steps = 23,
                modifier = Modifier.weight(1f).semantics { contentDescription = "Size" }.testTag("settings.fontSize"),
            )
            Icon(Icons.Outlined.TextIncrease, contentDescription = null)
        }
        Text(
            "The quick brown fox jumps over the lazy dog.", fontFamily = FontFamily.Serif,
            fontSize = ReaderPreferences.fontSize.sp, lineHeight = (ReaderPreferences.fontSize * 1.35f).sp)
    }
}

@Composable
private fun AnkiSettings(model: AppModel) {
    val store = model.config
    val config = store.config ?: return
    SettingsSection(footer = {
        val link = TextLinkStyles(
            SpanStyle(color = MaterialTheme.colorScheme.primary, textDecoration = TextDecoration.Underline))
        Text(
            buildAnnotatedString {
                append("Cards sync to Anki on your computer through the ")
                withLink(LinkAnnotation.Url("https://ankiweb.net/shared/info/2055492159", link)) { append("AnkiConnect") }
                append(
                    " add-on, every 5 minutes while FLTS runs. Enter the computer's address on your network, " +
                        "and let AnkiConnect accept connections from other devices.")
            },
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }) {
        SettingsSwitchRow(
            "Sync cards to Anki automatically", config.ankiSyncEnabled, Modifier.testTag("settings.ankiEnabled"),
        ) { on -> model.scope.launch { store.update { it.copy(ankiSyncEnabled = on) } } }
        // Anki runs on a computer; this device reaches it over the network.
        CommitField(
            title = "AnkiConnect address", value = config.ankiEndpoint ?: "", url = true,
            prompt = "http://192.168.1.10:8765", modifier = Modifier.testTag("settings.ankiEndpoint"),
        ) { v -> model.scope.launch { store.update { it.copy(ankiEndpoint = v.ifEmpty { null }) } } }
        CommitField(
            title = "AnkiConnect API key", value = config.ankiApiKey ?: "", secure = true,
            modifier = Modifier.testTag("settings.ankiApiKey"),
        ) { v -> model.scope.launch { store.update { it.copy(ankiApiKey = v.ifEmpty { null }) } } }
    }
    SettingsSection("Status") { AnkiStatusRow(model) }
}

@Composable
private fun AnkiStatusRow(model: AppModel) {
    val anki = model.anki
    val status = anki.status
    Row(verticalAlignment = Alignment.CenterVertically) {
        val (text, color) = when (status.state) {
            AnkiSyncState.IDLE -> "Not synced yet" to Color.Unspecified
            AnkiSyncState.SYNCING -> "Syncing…" to Color.Unspecified
            AnkiSyncState.OK -> {
                val r = status.lastReport
                val summary = when {
                    r == null -> "Synced"
                    r.failed > 0 -> "${r.succeeded} synced, ${r.failed} failed"
                    else -> "${r.totalCards} cards synced"
                }
                summary to SettingsGood
            }
            AnkiSyncState.ERR -> (status.lastError ?: "Failed") to MaterialTheme.colorScheme.error
            AnkiSyncState.UNREACHABLE -> "Anki is not running" to SettingsWarning
        }
        Text(text, color = color, modifier = Modifier.weight(1f).padding(end = 12.dp).testTag("settings.ankiStatus"))
        OutlinedButton(
            onClick = { model.scope.launch { anki.syncNow() } }, enabled = status.state != AnkiSyncState.SYNCING,
            modifier = Modifier.testTag("settings.ankiSyncNow"),
        ) { Text("Sync Now") }
    }
    anki.error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
}

@Composable
private fun StorageSettings(model: AppModel) {
    var root by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(model.core) { root = runCatching { model.core?.getLibraryRoot() }.getOrNull() }
    SettingsSection("Library") {
        Text("Location")
        HorizontalDivider()
        SelectionContainer {
            Text(
                root ?: "—", style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag("settings.libraryLocation"))
        }
    }
}
