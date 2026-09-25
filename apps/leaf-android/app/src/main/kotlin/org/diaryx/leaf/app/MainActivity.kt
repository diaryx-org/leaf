package org.diaryx.leaf.app

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.core.content.edit
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.diaryx.leaf.compose.LeafColors
import org.diaryx.leaf.compose.LeafEditor
import org.diaryx.leaf.compose.LeafFormattingBar
import org.diaryx.leaf.compose.LeafTheme

/**
 * Leaf: one document at a time, opened from the Storage Access Framework or
 * handed over by another app, edited in [LeafEditor] and saved back where it
 * came from — a second after the last edit, and whenever the app leaves the
 * screen.
 */
class MainActivity : ComponentActivity() {
    private var opened by mutableStateOf<Uri?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        opened = intent.data
        setContent { LeafApp(opened) }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.data?.let { opened = it }
    }
}

private const val PREFS = "leaf"
private const val LAST = "last-document"

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LeafApp(handedOver: Uri?) {
    val context = LocalContext.current
    val dark = isSystemInDarkTheme()
    val scheme = when {
        Build.VERSION.SDK_INT >= 31 && dark -> dynamicDarkColorScheme(context)
        Build.VERSION.SDK_INT >= 31 -> dynamicLightColorScheme(context)
        dark -> darkColorScheme()
        else -> lightColorScheme()
    }
    val prefs = remember { context.getSharedPreferences(PREFS, 0) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    var document by remember { mutableStateOf<Document?>(null) }
    var menu by remember { mutableStateOf(false) }
    var saveJob by remember { mutableStateOf<Job?>(null) }

    fun saveNow() {
        val d = document ?: return
        if (!d.editor.chrome.dirty) return
        runCatching { d.save(context) }.onFailure {
            scope.launch { snackbar.showSnackbar("Could not save ${d.name}: ${it.message}") }
        }
    }

    fun show(uri: Uri?) {
        saveNow()
        val next = runCatching { if (uri == null) Document.sample(context) else Document.open(context, uri) }
            .onFailure { scope.launch { snackbar.showSnackbar(it.message ?: "Could not open that document") } }
            .getOrNull() ?: return
        // Autosave: a second after the last edit, so a crash loses at most that.
        next.editor.onEdit = {
            saveJob?.cancel()
            saveJob = scope.launch {
                delay(1000)
                saveNow()
            }
        }
        document = next
        prefs.edit { putString(LAST, uri?.toString()) }
    }

    val openDocument = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri ?: return@rememberLauncherForActivityResult
        Document.persist(context, uri)
        show(uri)
    }
    val createDocument = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("text/markdown")) { uri ->
        uri ?: return@rememberLauncherForActivityResult
        Document.persist(context, uri)
        show(uri)
    }

    LaunchedEffect(handedOver) {
        val last = prefs.getString(LAST, null)?.let(Uri::parse)
        show(handedOver ?: last?.takeIf { runCatching { context.contentResolver.openInputStream(it)?.close() }.isSuccess })
    }
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) { saveNow() }

    MaterialTheme(colorScheme = scheme) {
        val theme = LeafTheme(
            colors = (if (dark) LeafColors.dark() else LeafColors.light()).copy(
                background = scheme.surface,
                caret = scheme.primary,
                handle = scheme.primary,
                selection = scheme.primary.copy(alpha = 0.3f),
                link = scheme.primary,
            ),
        )
        Scaffold(
            topBar = {
                TopAppBar(
                    title = {
                        val d = document
                        Text(
                            (d?.name ?: "Leaf") + if (d?.editor?.chrome?.dirty == true) " •" else "",
                            maxLines = 1, overflow = TextOverflow.Ellipsis,
                        )
                    },
                    actions = {
                        val d = document
                        if (d != null) {
                            TextButton({ d.editor.toggleView() }) {
                                Text(if (d.editor.chrome.view == "source") "Rendered" else "Source")
                            }
                        }
                        Box {
                            TextButton({ menu = true }) { Text("⋮") }
                            DropdownMenu(menu, { menu = false }) {
                                DropdownMenuItem({ Text("New") }, onClick = { menu = false; createDocument.launch("Untitled.md") })
                                DropdownMenuItem({ Text("Open…") }, onClick = {
                                    menu = false
                                    openDocument.launch(arrayOf("text/*", "application/octet-stream"))
                                })
                                DropdownMenuItem({ Text("Sample") }, onClick = { menu = false; show(null) })
                                if (d != null) {
                                    DropdownMenuItem({ Text("Word count") }, onClick = {
                                        menu = false
                                        val c = d.editor.doc.counts()
                                        scope.launch { snackbar.showSnackbar("${c.words} words, ${c.characters} characters") }
                                    })
                                }
                            }
                        }
                    },
                )
            },
            snackbarHost = { SnackbarHost(snackbar) },
        ) { inner ->
            Column(Modifier.fillMaxSize().padding(inner).consumeWindowInsets(inner).imePadding()) {
                val d = document
                if (d != null) {
                    // Keyed by the document, so a new one gets a fresh editor and scroll.
                    androidx.compose.runtime.key(d) {
                        LeafEditor(d.editor, Modifier.weight(1f).fillMaxWidth(), theme)
                        LeafFormattingBar(d.editor, Modifier.fillMaxWidth())
                    }
                }
            }
        }
    }
}
