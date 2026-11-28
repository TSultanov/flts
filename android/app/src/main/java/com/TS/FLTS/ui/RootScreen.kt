package com.TS.FLTS.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.TS.FLTS.stores.AppModel

@Composable
fun RootScreen(model: AppModel) {
    // Onboarding is for the first run only: once the app has been usable,
    // picking a provider without a key in Settings must not replace the screen.
    var wasConfigured by rememberSaveable { mutableStateOf(false) }
    Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        when (val phase = model.phase) {
            AppModel.Phase.Starting -> CenteredMessage(progress = true, title = "Opening library…")
            is AppModel.Phase.Failed ->
                CenteredMessage(progress = false, title = "FLTS could not start", detail = phase.message)
            AppModel.Phase.Running -> {
                val configured = model.config.isConfigured
                LaunchedEffect(configured) { if (configured) wasConfigured = true }
                when {
                    !model.config.loaded -> CenteredMessage(progress = true, title = "Opening library…")
                    !configured && !wasConfigured -> OnboardingScreen(model)
                    else -> MainScreen(model)
                }
            }
        }
    }
}

@Composable
fun CenteredMessage(progress: Boolean, title: String, detail: String? = null) {
    Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
            if (progress) {
                CircularProgressIndicator()
            } else {
                Icon(Icons.Outlined.WarningAmber, contentDescription = null, tint = MaterialTheme.colorScheme.error)
            }
            Text(title, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
            if (detail != null) {
                SelectionContainer {
                    Text(
                        detail, style = MaterialTheme.typography.bodyMedium, textAlign = TextAlign.Center,
                        color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}

/// The app's content: the library page, or the open book.
@Composable
private fun MainScreen(model: AppModel) {
    val bookId = model.selectedBookId
    if (bookId != null) {
        key(bookId) { BookPage(model, bookId) }
    } else {
        LibraryPage(model)
    }
    if (model.showImport) {
        ImportSheet(model, onDismiss = { model.showImport = false })
    }
    if (model.showSettings) {
        SettingsSheet(model, onDismiss = { model.showSettings = false })
    }
    // A remembered book from another library (or deleted elsewhere).
    val books = model.library.books
    LaunchedEffect(model.library.loaded, books) {
        val id = model.selectedBookId
        if (model.library.loaded && id != null && books.none { it.id == id }) model.selectedBookId = null
    }
    model.lastError?.let { message ->
        AlertDialog(
            onDismissRequest = { model.lastError = null },
            confirmButton = { TextButton(onClick = { model.lastError = null }) { Text("OK") } },
            title = { Text("Something went wrong") },
            text = { Text(message) },
        )
    }
}
