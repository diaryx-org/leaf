package org.diaryx.leaf.compose

import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Handler
import android.os.Looper
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import java.io.File
import java.util.concurrent.Executors

/**
 * The pictures a document's block images draw — leaf-swift's `MediaStore`,
 * for stills. Core holds no I/O and no path context, so a source is found
 * here: first by the host ([locate]), then against the document's own
 * directory ([base]). What is found is decoded off the main thread, once,
 * and [onLoaded] asks the editor to lay the box out at the picture's size.
 *
 * A source nothing can read — a remote URL, a file that is not there —
 * settles as nothing, and its placeholder row stands.
 */
internal class MediaStore(private val onLoaded: () -> Unit) {
    var base: File? = null
        set(value) {
            if (field != value) flush()
            field = value
        }
    var locate: ((String) -> File?)? = null

    /** Settled sources: a picture, or null for one that would not load. */
    private val settled = HashMap<String, ImageBitmap?>()
    private val pending = HashSet<String>()
    private var generation = 0
    private val main = Handler(Looper.getMainLooper())

    /** The picture for `src`, or null while it loads or when it cannot. Main thread. */
    fun still(src: String): ImageBitmap? {
        if (src in settled) return settled[src]
        if (src in pending) return null
        val file = resolve(src)
        if (file == null) {
            settled[src] = null
            return null
        }
        pending += src
        val asked = generation
        decoder.execute {
            val picture = runCatching { decode(file) }.getOrNull()
            main.post {
                if (asked != generation) return@post
                pending -= src
                settled[src] = picture
                if (picture != null) onLoaded()
            }
        }
        return null
    }

    /** Forget everything — a new document, or a directory that repoints every relative path. */
    fun flush() {
        generation++
        settled.clear()
        pending.clear()
    }

    private fun resolve(src: String): File? {
        locate?.invoke(src)?.let { return it.takeIf(File::isFile) }
        if ("://" in src || src.startsWith("data:")) return null
        val path = Uri.decode(src.substringBefore('#').substringBefore('?'))
        val file = if (path.startsWith("/")) File(path) else base?.resolve(path) ?: return null
        return file.takeIf(File::isFile)
    }

    private companion object {
        /** One decoder for every document: pictures load one at a time, in the order asked. */
        val decoder = Executors.newSingleThreadExecutor { Thread(it, "leaf-media").apply { isDaemon = true } }

        /** No picture is decoded past this on its longer side; a phone photo is four times it. */
        const val LONGEST = 2048

        fun decode(file: File): ImageBitmap? {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeFile(file.path, bounds)
            if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
            var sample = 1
            while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= LONGEST) sample *= 2
            val options = BitmapFactory.Options().apply { inSampleSize = sample }
            return BitmapFactory.decodeFile(file.path, options)?.asImageBitmap()
        }
    }
}
