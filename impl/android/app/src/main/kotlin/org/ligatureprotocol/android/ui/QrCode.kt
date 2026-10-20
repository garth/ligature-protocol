package org.ligatureprotocol.android.ui

import android.graphics.Bitmap
import android.graphics.Color
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel

/** The newcomer's offer as a QR code, drawn with ZXing (no Google services). */
@Composable
fun QrCode(text: String, modifier: Modifier = Modifier) {
    val bitmap = remember(text) { render(text) }
    Image(
        bitmap = bitmap.asImageBitmap(),
        contentDescription = "Pairing offer",
        contentScale = ContentScale.Fit,
        filterQuality = FilterQuality.None,
        modifier = modifier
            .fillMaxWidth()
            .aspectRatio(1f)
            .background(androidx.compose.ui.graphics.Color.White)
            .padding(8.dp),
    )
}

private fun render(text: String): Bitmap {
    val hints = mapOf(
        EncodeHintType.MARGIN to 1,
        EncodeHintType.ERROR_CORRECTION to ErrorCorrectionLevel.M,
    )
    val matrix = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, 0, 0, hints)
    val w = matrix.width
    val h = matrix.height
    val pixels = IntArray(w * h) { i ->
        if (matrix.get(i % w, i / w)) Color.BLACK else Color.WHITE
    }
    return Bitmap.createBitmap(pixels, w, h, Bitmap.Config.RGB_565)
}
