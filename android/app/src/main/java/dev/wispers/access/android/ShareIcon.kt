package dev.wispers.access.android

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import androidx.core.content.res.ResourcesCompat
import androidx.core.graphics.drawable.IconCompat

// Wispers key colour (light sage) with a tone-on-tone dark-sage letter.
private const val TILE_BACKGROUND = 0xFFA1D283.toInt() // AccessPrimaryLight
private const val TILE_LETTER = 0xFF4A6B36.toInt()     // AccessPrimaryDark

// 108dp adaptive-icon canvas at ~xxxhdpi. The launcher masks the outer ring, so
// the full-bleed background bleeds safely and the centred letter — at half the
// canvas height — stays comfortably inside the safe zone.
private const val TILE_SIZE = 432
private const val LETTER_FRACTION = 0.5f

/** First letter of [nickname], lowercased; falls back to 'w' (as in the logo). */
private fun tileLetter(nickname: String): Char =
    nickname.firstOrNull { it.isLetter() }?.lowercaseChar() ?: 'w'

/** Renders a share's sage letter tile: a lowercase DM Serif initial on light sage. */
fun shareLetterTile(context: Context, nickname: String): Bitmap {
    val bitmap = Bitmap.createBitmap(TILE_SIZE, TILE_SIZE, Bitmap.Config.ARGB_8888)
    val canvas = Canvas(bitmap)
    canvas.drawColor(TILE_BACKGROUND)
    val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = TILE_LETTER
        typeface = ResourcesCompat.getFont(context, R.font.dm_serif_text_regular) ?: Typeface.SERIF
        textAlign = Paint.Align.CENTER
        textSize = TILE_SIZE * LETTER_FRACTION
    }
    val metrics = paint.fontMetrics
    val baseline = TILE_SIZE / 2f - (metrics.ascent + metrics.descent) / 2f
    canvas.drawText(tileLetter(nickname).toString(), TILE_SIZE / 2f, baseline, paint)
    return bitmap
}

/**
 * Adaptive home-screen-shortcut icon for a share. Phase 2 will prefer an icon
 * harvested from the site (manifest/favicon) and fall back to this tile.
 */
fun shareIcon(context: Context, nickname: String): IconCompat =
    IconCompat.createWithAdaptiveBitmap(shareLetterTile(context, nickname))
