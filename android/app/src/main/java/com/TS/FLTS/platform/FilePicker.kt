package com.TS.FLTS.platform

import android.content.Context
import android.net.Uri
import com.TS.FLTS.BuildConfig
import java.io.File
import java.io.IOException
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

object FilePicker {
    /// Many providers label an EPUB as a plain binary.
    val epubMimeTypes = arrayOf("application/epub+zip", "application/octet-stream")

    /// A debug build's UI tests cannot drive the system picker: they name the file here.
    val uiTestEpub: String?
        get() = if (BuildConfig.DEBUG) System.getenv("FLTS_UITEST_EPUB")?.takeIf { it.isNotEmpty() } else null

    private fun importDir(context: Context) = File(context.cacheDir, "import")

    /// The core reads a filesystem path, and a picked document is only a stream.
    suspend fun copyToCache(context: Context, uri: Uri): File = withContext(Dispatchers.IO) {
        val dir = importDir(context).apply { mkdirs() }
        val file = File(dir, "${UUID.randomUUID()}.epub")
        try {
            val input = context.contentResolver.openInputStream(uri) ?: throw IOException("Cannot open $uri")
            input.use { source -> file.outputStream().use { source.copyTo(it) } }
            file
        } catch (e: Throwable) {
            file.delete()
            throw e
        }
    }

    suspend fun clearImportCache(context: Context) = withContext(Dispatchers.IO) {
        importDir(context).listFiles()?.forEach { it.delete() }
    }
}
