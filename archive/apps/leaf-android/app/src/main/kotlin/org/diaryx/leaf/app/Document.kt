package org.diaryx.leaf.app

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import org.diaryx.leaf.compose.LeafEditorState
import uniffi.leaf_ffi.LeafDoc
import uniffi.leaf_ffi.LeafException

/**
 * One open document: where its bytes live, what format they are in, and the
 * editor over them. The bytes live behind a `content://` URI the Storage
 * Access Framework granted (or a file of the app's own, for the sample), and
 * are written back there whole.
 */
class Document(val uri: Uri, val name: String, val editor: LeafEditorState) {
    fun save(context: Context) {
        val text = editor.source()
        context.contentResolver.openOutputStream(uri, "wt")!!.use { it.write(text.toByteArray()) }
        editor.markSaved()
    }

    companion object {
        /** leaf's format name for a file name — Markdown unless its extension says otherwise. */
        fun format(name: String): String = when (name.substringAfterLast('.', "").lowercase()) {
            "dj", "djot" -> "djot"
            "html", "htm" -> "html"
            "xml" -> "xml"
            else -> "markdown"
        }

        fun open(context: Context, uri: Uri): Document {
            val name = displayName(context, uri)
            val text = context.contentResolver.openInputStream(uri)!!.use { it.readBytes().decodeToString() }
            val doc = try {
                LeafDoc(text, format(name))
            } catch (e: LeafException) {
                val why = (e as? LeafException.Parse)?.reason ?: e.message
                throw IllegalArgumentException("$name could not be read: $why", e)
            }
            return Document(uri, name, LeafEditorState(doc))
        }

        /** The bundled sample, copied out once so it is a file that can be edited and saved. */
        fun sample(context: Context): Document {
            val file = context.filesDir.resolve("Sample.md")
            if (!file.exists()) {
                context.assets.open("Sample.md").use { input -> file.outputStream().use { input.copyTo(it) } }
            }
            return open(context, Uri.fromFile(file))
        }

        private fun displayName(context: Context, uri: Uri): String {
            if (uri.scheme == "file") return uri.lastPathSegment ?: "Untitled.md"
            context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst()) c.getString(0)?.let { return it }
            }
            return uri.lastPathSegment ?: "Untitled.md"
        }

        /** Keep a grant across restarts, where the provider offers one. */
        fun persist(context: Context, uri: Uri) {
            if (uri.scheme != "content") return
            runCatching {
                context.contentResolver.takePersistableUriPermission(
                    uri, Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                )
            }
        }
    }
}
