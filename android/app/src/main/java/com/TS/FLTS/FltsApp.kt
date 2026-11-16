package com.TS.FLTS

import android.app.Application
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import com.TS.FLTS.stores.AppModel

/// Holds the app model, so the core outlives any activity.
class FltsApp : Application() {
    val model by lazy { AppModel() }

    override fun onCreate() {
        super.onCreate()
        ProcessLifecycleOwner.get().lifecycle.addObserver(
            object : DefaultLifecycleObserver {
                override fun onStart(owner: LifecycleOwner) = model.foregrounded()
            },
        )
    }
}
