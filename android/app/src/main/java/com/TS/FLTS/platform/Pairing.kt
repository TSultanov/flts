package com.TS.FLTS.platform

import com.TS.FLTS.ui.DialogSystemBars
import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Color
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.NoPhotography
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.EncodeHintType
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.ReaderException
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import com.google.zxing.qrcode.QRCodeWriter
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/// One pixel per module: draw it scaled with FilterQuality.None. Always black
/// on white with a quiet zone, so it scans on a dark theme too.
fun qrCode(payload: String): ImageBitmap? = try {
    val hints = mapOf(
        EncodeHintType.ERROR_CORRECTION to ErrorCorrectionLevel.M,
        EncodeHintType.MARGIN to 2,
    )
    // Raw UTF-8 bytes with no ECI header, as the iOS generator draws them.
    val bytes = String(payload.toByteArray(Charsets.UTF_8), Charsets.ISO_8859_1)
    val matrix = QRCodeWriter().encode(bytes, BarcodeFormat.QR_CODE, 0, 0, hints)
    val pixels = IntArray(matrix.width * matrix.height) { i ->
        if (matrix.get(i % matrix.width, i / matrix.width)) Color.BLACK else Color.WHITE
    }
    Bitmap.createBitmap(pixels, matrix.width, matrix.height, Bitmap.Config.ARGB_8888).asImageBitmap()
} catch (e: Exception) {
    null
}

/// False on devices without any camera.
fun pairingScannerSupported(context: Context): Boolean =
    context.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)

/// Scans another device's pairing QR code with the camera.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PairingScanner(found: (String) -> Unit, onDismiss: () -> Unit) {
    val context = LocalContext.current
    // null until the camera permission is known.
    var allowed by remember { mutableStateOf<Boolean?>(null) }
    val request = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { allowed = it }
    LaunchedEffect(Unit) {
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            allowed = true
        } else {
            request.launch(Manifest.permission.CAMERA)
        }
    }
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        DialogSystemBars()
        Scaffold(
            modifier = Modifier.fillMaxSize().testTag("scanner"),
            topBar = {
                TopAppBar(
                    title = { Text("Scan Pairing Code") },
                    navigationIcon = {
                        TextButton(onClick = onDismiss, modifier = Modifier.testTag("scanner.cancel")) { Text("Cancel") }
                    },
                )
            },
        ) { padding ->
            Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
                when (allowed) {
                    null -> CircularProgressIndicator()
                    true -> CameraScanner(
                        found = { code ->
                            found(code)
                            onDismiss()
                        },
                        failed = { allowed = false },
                    )
                    false -> CameraUnavailable()
                }
            }
        }
    }
}

@Composable
private fun CameraUnavailable() {
    Column(
        Modifier.padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(Icons.Outlined.NoPhotography, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Text("Camera Unavailable", style = MaterialTheme.typography.titleMedium)
        Text(
            "Allow camera access for FLTS in Settings, or enter the device ID by hand.",
            style = MaterialTheme.typography.bodyMedium, textAlign = TextAlign.Center,
            color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun CameraScanner(found: (String) -> Unit, failed: () -> Unit) {
    val context = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val onFound by rememberUpdatedState(found)
    val onFailed by rememberUpdatedState(failed)
    val previewView = remember {
        // A TextureView: a SurfaceView would punch through the dialog's window.
        PreviewView(context).apply { implementationMode = PreviewView.ImplementationMode.COMPATIBLE }
    }
    DisposableEffect(owner) {
        val executor = Executors.newSingleThreadExecutor()
        val main = ContextCompat.getMainExecutor(context)
        val future = ProcessCameraProvider.getInstance(context)
        var provider: ProcessCameraProvider? = null
        var disposed = false
        val done = AtomicBoolean(false)
        val analysis = ImageAnalysis.Builder()
            .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
            .build()
        analysis.setAnalyzer(executor, QrAnalyzer { text ->
            // Frames keep coming until the dialog is gone; report the first only.
            if (done.compareAndSet(false, true)) main.execute { if (!disposed) onFound(text) }
        })
        future.addListener({
            if (!disposed) {
                try {
                    val cameras = future.get()
                    provider = cameras
                    val preview = Preview.Builder().build()
                    preview.setSurfaceProvider(previewView.surfaceProvider)
                    val selector =
                        if (cameras.hasCamera(CameraSelector.DEFAULT_BACK_CAMERA)) CameraSelector.DEFAULT_BACK_CAMERA
                        else CameraSelector.DEFAULT_FRONT_CAMERA
                    cameras.unbindAll()
                    cameras.bindToLifecycle(owner, selector, preview, analysis)
                } catch (e: Exception) {
                    onFailed()
                }
            }
        }, main)
        onDispose {
            disposed = true
            analysis.clearAnalyzer()
            provider?.unbindAll()
            executor.shutdown()
        }
    }
    AndroidView(factory = { previewView }, modifier = Modifier.fillMaxSize().testTag("scanner.preview"))
}

private class QrAnalyzer(private val found: (String) -> Unit) : ImageAnalysis.Analyzer {
    private val reader = QRCodeReader()
    // Codes drawn by iOS carry raw UTF-8 with no ECI marker.
    private val hints = mapOf(DecodeHintType.CHARACTER_SET to "UTF-8")
    private var luminance = ByteArray(0)

    override fun analyze(image: ImageProxy) {
        try {
            // The Y plane of YUV_420_888. Rows are rowStride wide, and the last
            // one may stop at the image's width.
            val plane = image.planes[0]
            val stride = plane.rowStride
            val size = stride * image.height
            if (luminance.size != size) luminance = ByteArray(size)
            val data = plane.buffer
            data.rewind()
            data.get(luminance, 0, minOf(size, data.remaining()))
            val source = PlanarYUVLuminanceSource(luminance, stride, image.height, 0, 0, image.width, image.height, false)
            val text = reader.decode(BinaryBitmap(HybridBinarizer(source)), hints).text
            if (!text.isNullOrEmpty()) found(text)
        } catch (e: ReaderException) {
            // No code in this frame.
        } catch (e: Exception) {
            // A frame that cannot be read is skipped like an empty one.
        } finally {
            reader.reset()
            image.close()
        }
    }
}
