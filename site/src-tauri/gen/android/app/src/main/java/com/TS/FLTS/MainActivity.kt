package com.TS.FLTS

import android.graphics.Color
import android.os.Bundle
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // The app's top bar (Nav) is always dark, so force light (white) status-bar
    // icons regardless of the system light/dark theme. Plain enableEdgeToEdge()
    // uses `auto`, which picks dark icons in light mode — invisible on our bar.
    enableEdgeToEdge(
      statusBarStyle = SystemBarStyle.dark(Color.TRANSPARENT)
    )
    super.onCreate(savedInstanceState)
  }
}
