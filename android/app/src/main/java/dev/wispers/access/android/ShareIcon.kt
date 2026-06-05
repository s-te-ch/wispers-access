package dev.wispers.access.android

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
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

/** Decodes the cached PNG, or null if absent / undecodable. */
private fun decodeCached(iconPng: ByteArray?): Bitmap? =
    iconPng?.let { BitmapFactory.decodeByteArray(it, 0, it.size) }

/** The share's display bitmap: the harvested site icon if cached, else the letter tile. */
fun shareBitmap(context: Context, nickname: String, iconPng: ByteArray?): Bitmap =
    decodeCached(iconPng) ?: shareLetterTile(context, nickname)

/**
 * Home-screen-shortcut icon for a share. A harvested site icon is used as a
 * legacy (standalone) icon; the generated tile is full-bleed adaptive.
 */
fun shareIcon(context: Context, nickname: String, iconPng: ByteArray?): IconCompat {
    val cached = decodeCached(iconPng)
    return if (cached != null) {
        IconCompat.createWithBitmap(cached)
    } else {
        IconCompat.createWithAdaptiveBitmap(shareLetterTile(context, nickname))
    }
}
