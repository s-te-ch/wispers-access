package dev.wispers.access.android.screens

import android.graphics.Rect
import android.media.Image
import androidx.camera.core.ImageInfo
import androidx.camera.core.ImageProxy
import com.google.zxing.BarcodeFormat
import com.google.zxing.common.BitMatrix
import com.google.zxing.qrcode.QRCodeWriter
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.ByteBuffer

/**
 * Feeds [QrCodeAnalyzer] luminance frames shaped like CameraX's YUV_420_888
 * analysis frames, with QR codes drawn by ZXing's own encoder.
 */
class QrCodeAnalyzerTest {
    private val invite = "wa1-demo-invite-0123456789abcdefghijklmnopqrstuvwxyz"

    @Test
    fun decodesQrCodeInFrameWithPaddedRows() {
        val frame = frameWithCode(qrCode(invite), rowPadding = 32)
        assertEquals(listOf(invite), analyze(frame))
        assertTrue(frame.closed)
    }

    @Test
    fun decodesLightOnDarkQrCode() {
        assertEquals(listOf(invite), analyze(frameWithCode(qrCode(invite), inverted = true)))
    }

    @Test
    fun decodesSidewaysQrCode() {
        assertEquals(listOf(invite), analyze(frameWithCode(rotatedQuarterTurn(qrCode(invite)))))
    }

    @Test
    fun reportsNothingForFrameWithoutQrCode() {
        val frame = FakeFrame(640, 480, 640, ByteArray(640 * 480) { 0x80.toByte() })
        assertEquals(emptyList<String>(), analyze(frame))
        assertTrue(frame.closed)
    }

    @Test
    fun decodesAgainAfterFrameWithoutQrCode() {
        val analyzer = collectingAnalyzer()
        analyzer.first.analyze(FakeFrame(640, 480, 640, ByteArray(640 * 480)))
        analyzer.first.analyze(frameWithCode(qrCode(invite)))
        assertEquals(listOf(invite), analyzer.second)
    }

    private fun analyze(frame: FakeFrame): List<String> {
        val (analyzer, decoded) = collectingAnalyzer()
        analyzer.analyze(frame)
        return decoded
    }

    private fun collectingAnalyzer(): Pair<QrCodeAnalyzer, MutableList<String>> {
        val decoded = mutableListOf<String>()
        return QrCodeAnalyzer { decoded += it } to decoded
    }

    private fun qrCode(text: String): BitMatrix = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, 300, 300)

    /**
     * A 640×480 frame with [code] in the middle, rows padded by [rowPadding]
     * bytes as some camera HALs do. The last row stops at the width, as a real
     * plane buffer may.
     */
    private fun frameWithCode(code: BitMatrix, rowPadding: Int = 0, inverted: Boolean = false): FakeFrame {
        val width = 640
        val height = 480
        val rowStride = width + rowPadding
        val dark: Byte = if (inverted) 0xE0.toByte() else 0x20
        val light: Byte = if (inverted) 0x20 else 0xE0.toByte()
        val luminance = ByteArray(rowStride * (height - 1) + width) { light }
        val left = (width - code.width) / 2
        val top = (height - code.height) / 2
        for (y in 0 until code.height) {
            for (x in 0 until code.width) {
                if (code[x, y]) luminance[(top + y) * rowStride + left + x] = dark
            }
        }
        return FakeFrame(width, height, rowStride, luminance)
    }

    private fun rotatedQuarterTurn(code: BitMatrix): BitMatrix {
        val turned = BitMatrix(code.height, code.width)
        for (y in 0 until code.height) {
            for (x in 0 until code.width) {
                if (code[x, y]) turned.set(code.height - 1 - y, x)
            }
        }
        return turned
    }

    /** Only what the analyzer reads: the size and the Y plane. */
    private class FakeFrame(
        private val frameWidth: Int,
        private val frameHeight: Int,
        private val frameRowStride: Int,
        luminance: ByteArray,
    ) : ImageProxy {
        var closed = false
            private set
        private val plane = object : ImageProxy.PlaneProxy {
            private val buffer = ByteBuffer.wrap(luminance)
            override fun getRowStride(): Int = frameRowStride
            override fun getPixelStride(): Int = 1
            override fun getBuffer(): ByteBuffer = buffer
        }

        override fun getWidth(): Int = frameWidth
        override fun getHeight(): Int = frameHeight
        override fun getPlanes() = arrayOf<ImageProxy.PlaneProxy>(plane)
        override fun close() {
            closed = true
        }
        override fun getFormat(): Int = throw UnsupportedOperationException()
        override fun getCropRect(): Rect = throw UnsupportedOperationException()
        override fun setCropRect(rect: Rect?): Unit = throw UnsupportedOperationException()
        override fun getImageInfo(): ImageInfo = throw UnsupportedOperationException()
        override fun getImage(): Image = throw UnsupportedOperationException()
    }
}
