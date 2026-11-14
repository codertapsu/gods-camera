package dev.godscamera.android.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathBuilder
import androidx.compose.ui.graphics.vector.path
import androidx.compose.ui.unit.dp

/**
 * The few pictures the screens need that the stock set does not have. Each is
 * drawn as lines two units wide on a 24-unit square, like the stock ones, and
 * takes the colour of whatever shows it.
 */
object AppIcons {
    private fun lines(name: String, draw: PathBuilder.() -> Unit): ImageVector =
        ImageVector.Builder(name, 24.dp, 24.dp, 24f, 24f).apply {
            path(
                stroke = SolidColor(Color.Black), strokeLineWidth = 2f,
                strokeLineCap = StrokeCap.Round, strokeLineJoin = StrokeJoin.Round, pathBuilder = draw,
            )
        }.build()

    /** A laptop: the Mac. */
    val Mac: ImageVector by lazy {
        lines("Mac") {
            // The screen.
            moveTo(5f, 5f); lineTo(19f, 5f); lineTo(19f, 15f); lineTo(5f, 15f); close()
            // The base it stands on.
            moveTo(2.5f, 19f); lineTo(21.5f, 19f)
        }
    }

    /** A page inside the corners of a viewfinder: scanning a document. */
    val Scan: ImageVector by lazy {
        lines("Scan") {
            // Four corners.
            moveTo(3f, 8f); lineTo(3f, 4f); lineTo(7f, 4f)
            moveTo(17f, 4f); lineTo(21f, 4f); lineTo(21f, 8f)
            moveTo(21f, 16f); lineTo(21f, 20f); lineTo(17f, 20f)
            moveTo(7f, 20f); lineTo(3f, 20f); lineTo(3f, 16f)
            // Lines of text.
            moveTo(8f, 9f); lineTo(16f, 9f)
            moveTo(8f, 12f); lineTo(16f, 12f)
            moveTo(8f, 15f); lineTo(13f, 15f)
        }
    }

    /** Two pictures, one behind the other: choosing photos. */
    val Photos: ImageVector by lazy {
        lines("Photos") {
            // The picture in front.
            moveTo(3f, 8f); lineTo(17f, 8f); lineTo(17f, 20f); lineTo(3f, 20f); close()
            // The edge of the one behind.
            moveTo(7f, 4f); lineTo(21f, 4f); lineTo(21f, 16f)
            // A hill in the picture.
            moveTo(5.5f, 17f); lineTo(9f, 13f); lineTo(11.5f, 15.5f); lineTo(13f, 14f); lineTo(15f, 17f)
        }
    }

    /** Two sheets, one behind the other: copying. */
    val Copy: ImageVector by lazy {
        lines("Copy") {
            moveTo(9f, 8f); lineTo(20f, 8f); lineTo(20f, 21f); lineTo(9f, 21f); close()
            moveTo(5f, 16f); lineTo(4f, 16f); lineTo(4f, 3f); lineTo(15f, 3f); lineTo(15f, 4f)
        }
    }

    /** A page with lines of text: a document. */
    val Page: ImageVector by lazy {
        lines("Page") {
            moveTo(6f, 3f); lineTo(14f, 3f); lineTo(18f, 7f); lineTo(18f, 21f); lineTo(6f, 21f); close()
            moveTo(9f, 11f); lineTo(15f, 11f)
            moveTo(9f, 14f); lineTo(15f, 14f)
            moveTo(9f, 17f); lineTo(13f, 17f)
        }
    }

    /** A ring around a dot: recording. */
    val Record: ImageVector by lazy {
        lines("Record") {
            moveTo(20f, 12f); arcTo(8f, 8f, 0f, true, true, 4f, 12f); arcTo(8f, 8f, 0f, true, true, 20f, 12f)
            moveTo(14.5f, 12f); arcTo(2.5f, 2.5f, 0f, true, true, 9.5f, 12f); arcTo(2.5f, 2.5f, 0f, true, true, 14.5f, 12f)
        }
    }

    /** A square: stopping. */
    val Stop: ImageVector by lazy {
        lines("Stop") {
            moveTo(7f, 7f); lineTo(17f, 7f); lineTo(17f, 17f); lineTo(7f, 17f); close()
        }
    }

    /** A video camera: the live picture. */
    val Live: ImageVector by lazy {
        lines("Live") {
            moveTo(3f, 7f); lineTo(15f, 7f); lineTo(15f, 17f); lineTo(3f, 17f); close()
            moveTo(15f, 10.5f); lineTo(21f, 7.5f); lineTo(21f, 16.5f); lineTo(15f, 13.5f)
        }
    }
}
