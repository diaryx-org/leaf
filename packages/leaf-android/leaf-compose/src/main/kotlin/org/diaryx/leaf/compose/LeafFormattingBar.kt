package org.diaryx.leaf.compose

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * The formatting controls a phone editor keeps above its keyboard: history,
 * the inline marks, the block styles, lists and nesting, and a link. Each is
 * lit from [LeafEditorChrome] — what the caret is standing in — and dimmed
 * where the document's format cannot spell it ([LeafEditorState.capabilities]).
 *
 * One scrolling row, since a phone is narrower than the controls are many.
 * The glyphs are type rather than icons, which is what a formatting bar has
 * always drawn them as and needs no icon set to ship.
 */
@Composable
fun LeafFormattingBar(state: LeafEditorState, modifier: Modifier = Modifier) {
    val chrome = state.chrome
    val caps = state.capabilities
    val source = chrome.view == "source"
    var styleMenu by remember { mutableStateOf(false) }
    var linkDialog by remember { mutableStateOf(false) }

    Surface(modifier, tonalElevation = 3.dp) {
        Row(
            Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 6.dp, vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Key("↶", "Undo", enabled = chrome.canUndo) { state.undo() }
            Key("↷", "Redo", enabled = chrome.canRedo) { state.redo() }
            Divider()
            Box {
                Key(
                    chrome.heading?.let { "H$it" } ?: "¶", "Style",
                    enabled = caps.heading && !source, lit = chrome.heading != null,
                ) { styleMenu = true }
                DropdownMenu(styleMenu, onDismissRequest = { styleMenu = false }) {
                    DropdownMenuItem({ Text("Body") }, onClick = { styleMenu = false; state.command { it.setParagraph() } })
                    for (level in 1..3) {
                        DropdownMenuItem(
                            { Text("Heading $level", fontWeight = FontWeight.Bold, fontSize = (22 - level * 2).sp) },
                            onClick = { styleMenu = false; state.command { it.setHeading(level.toUInt()) } },
                        )
                    }
                }
            }
            Key("B", "Bold", caps.bold && !source, "bold" in chrome.active, TextStyle(fontWeight = FontWeight.Bold)) {
                state.command { it.toggleBold() }
            }
            Key("I", "Italic", caps.italic && !source, "italic" in chrome.active, TextStyle(fontStyle = FontStyle.Italic, fontFamily = FontFamily.Serif)) {
                state.command { it.toggleItalic() }
            }
            Key("S", "Strikethrough", caps.strike && !source, "strike" in chrome.active, TextStyle(textDecoration = TextDecoration.LineThrough)) {
                state.command { it.toggleStrike() }
            }
            Key("‹›", "Code", caps.code && !source, "code" in chrome.active, TextStyle(fontFamily = FontFamily.Monospace)) {
                state.command { it.toggleCode() }
            }
            Key("✎", "Highlight", caps.mark && !source, "mark" in chrome.active) { state.command { it.toggleMark() } }
            Key("🔗", "Link", caps.link && !source, chrome.link != null) { linkDialog = true }
            Divider()
            Key("•", "Bulleted list", caps.bulletList && !source) { state.command { it.toggleList(false) } }
            Key("1.", "Numbered list", caps.orderedList && !source) { state.command { it.toggleList(true) } }
            Key("☑", "Checklist", caps.task && !source, chrome.task != null) { state.command { it.toggleTaskItem() } }
            Key("❝", "Block quote", caps.blockquote && !source, chrome.blockquote) { state.command { it.toggleBlockquote() } }
            Key("{ }", "Code block", caps.codeBlock && !source, chrome.codeBlock, TextStyle(fontFamily = FontFamily.Monospace)) {
                state.command { it.toggleCodeBlock() }
            }
            Divider()
            Key("⇤", "Outdent", !source) { state.command { it.outdent() } }
            Key("⇥", "Indent", !source) { state.command { it.indent() } }
            Key("―", "Horizontal rule", caps.thematicBreak && !source) { state.command { it.insertThematicBreak() } }
        }
    }

    if (linkDialog) {
        LinkDialog(
            initial = chrome.link ?: "",
            onDismiss = { linkDialog = false },
            onConfirm = { dest ->
                linkDialog = false
                if (dest.isNotBlank()) state.command { it.insertLink(dest.trim()) }
            },
        )
    }
}

@Composable
private fun Key(
    glyph: String,
    label: String,
    enabled: Boolean = true,
    lit: Boolean = false,
    style: TextStyle = TextStyle(),
    onClick: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    Box(
        Modifier
            .defaultMinSize(minWidth = 40.dp)
            .height(40.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(if (lit) colors.secondaryContainer else colors.surface.copy(alpha = 0f))
            .clickable(enabled = enabled, onClick = onClick)
            .semantics {
                contentDescription = label
                role = Role.Button
                selected = lit
            }
            .alpha(if (enabled) 1f else 0.35f)
            .padding(horizontal = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            glyph,
            style = style.merge(TextStyle(fontSize = 18.sp)),
            color = if (lit) colors.onSecondaryContainer else colors.onSurface,
        )
    }
}

@Composable
private fun Divider() {
    Spacer(Modifier.width(1.dp).height(24.dp).background(MaterialTheme.colorScheme.outlineVariant))
}

/** Where the link goes — seeded with the link the caret stands in, so it repoints that one. */
@Composable
private fun LinkDialog(initial: String, onDismiss: () -> Unit, onConfirm: (String) -> Unit) {
    var text by remember { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (initial.isEmpty()) "Add link" else "Edit link") },
        text = {
            OutlinedTextField(text, { text = it }, label = { Text("URL") }, singleLine = true)
        },
        confirmButton = { TextButton({ onConfirm(text) }) { Text("OK") } },
        dismissButton = { TextButton(onDismiss) { Text("Cancel") } },
    )
}
