package com.autocut.app.ui

import android.view.View
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.updatePadding

/**
 * Keeps a view clear of the navigation bar.
 *
 * targetSdk 35 makes edge-to-edge mandatory on Android 15: the window spans the
 * whole display and `android:navigationBarColor` stops doing anything. Without
 * this the bottom row of buttons drew underneath the navigation bar. The top is
 * handled by `fitsSystemWindows` on the CoordinatorLayout, which is what
 * AppBarLayout reads.
 *
 * The original padding is captured once, so repeated inset passes add the bar
 * height rather than accumulating it.
 */
fun View.padForBottomSystemBar() {
    val initialBottom = paddingBottom
    ViewCompat.setOnApplyWindowInsetsListener(this) { view, insets ->
        val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
        view.updatePadding(bottom = initialBottom + bars.bottom)
        // Returned unconsumed so siblings still see them.
        insets
    }
}
