package org.diaryx.leaf.app

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CompletableDeferred
import org.diaryx.leaf.compose.LeafDirectiveAnswer
import org.diaryx.leaf.compose.LeafDirectiveContent
import org.diaryx.leaf.compose.LeafDirectiveItem
import uniffi.leaf_ffi.DirectiveAttr

// `::embed{src=…}`, the one directive this app draws and offers — a
// demonstration of leaf's directive hook, not a vocabulary: the same titled
// card leaf's other apps draw, so each shows the hook the same way.

/** The app's drawing of its directives: an `embed` with a `src` is a card, anything else the placeholder. */
val embedContent: LeafDirectiveContent = { view ->
    if (view.name != "embed") null
    else view.attrs.firstOrNull { it.key == "src" }?.value?.takeIf { it.isNotEmpty() }?.let { src -> { EmbedCard(src) } }
}

/** An embed, drawn as a card: bordered, "Embed", and the URL beneath. */
@Composable
fun EmbedCard(src: String) {
    Surface(
        Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(10.dp),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
        color = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        Column(Modifier.padding(horizontal = 14.dp, vertical = 12.dp)) {
            Text("Embed", style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)
            Text(
                src,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

/**
 * The Insert menu's "Embed…" ([item]), which asks for a URL: [pending] is the
 * question while it is open, and [EmbedDialog] shows it and answers it.
 */
class EmbedAsk {
    var pending by mutableStateOf<CompletableDeferred<String?>?>(null)
        private set

    val item = LeafDirectiveItem(
        id = "directive.embed",
        name = "embed",
        title = "Embed…",
        icon = Icons.Outlined.Share,
        ask = {
            // One question at a time: a new one cancels any still open.
            pending?.complete(null)
            val question = CompletableDeferred<String?>()
            pending = question
            try {
                question.await()?.trim()?.takeIf { it.isNotEmpty() }?.let {
                    LeafDirectiveAnswer(attrs = listOf(DirectiveAttr("src", it)))
                }
            } finally {
                if (pending === question) pending = null
            }
        },
    )
}

/** Asks for an embed's URL while [ask] has a question pending. */
@Composable
fun EmbedDialog(ask: EmbedAsk) {
    val question = ask.pending ?: return
    var text by remember(question) { mutableStateOf("https://") }
    AlertDialog(
        onDismissRequest = { question.complete(null) },
        title = { Text("Embed") },
        text = { OutlinedTextField(text, { text = it }, label = { Text("URL") }, singleLine = true) },
        confirmButton = { TextButton({ question.complete(text) }) { Text("Insert") } },
        dismissButton = { TextButton({ question.complete(null) }) { Text("Cancel") } },
    )
}
