//! Opening and saving, for a page with no files of its own.
//!
//! - **The page remembers.** Every change is kept in the browser's
//!   `localStorage`, half a second after the last, and the page opens on
//!   what it kept rather than on the demo — so a reload loses nothing.
//!   `?demo` in the address opens the demo again. Dropped pictures are kept
//!   in memory only, and a reload draws them as chips.
//! - **⌘S** writes the document to a file the person picks, through the
//!   browser's save picker where it has one (Chrome, Edge), and to the same
//!   file at every ⌘S after; elsewhere it downloads it. ⇧⌘S, or a document
//!   opened by dropping, asks again.
//! - **⌘O**, or a document file dropped on the page, opens one: Markdown,
//!   Djot, HTML, AsciiDoc or XML, by its extension, in place of what was
//!   open.
//!
//! The pickers (`showSaveFilePicker`, `showOpenFilePicker`) are called by
//! name rather than through `web-sys`, which has them only behind its
//! unstable APIs.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use js_sys::{Array, Function, Object, Reflect};
use leaf_core::{Doc, Format};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::web::Ed;

/// Where the page keeps the document between visits.
const KEPT: &str = "leaf:document";
/// What the save picker suggests for a document with no name yet.
const UNTITLED: &str = "Untitled.md";
/// The extensions the open picker offers, and what each opens as.
const FORMATS: [(&str, Format); 9] = [
    ("md", Format::Markdown),
    ("markdown", Format::Markdown),
    ("dj", Format::Djot),
    ("djot", Format::Djot),
    ("html", Format::Html),
    ("htm", Format::Html),
    ("adoc", Format::Asciidoc),
    ("asciidoc", Format::Asciidoc),
    ("xml", Format::Xml),
];

/// The format a file's name says it is in, if it is one leaf opens.
pub fn format_of(name: &str) -> Option<Format> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    FORMATS.iter().find(|(e, _)| *e == ext).map(|(_, f)| *f)
}

#[derive(Default)]
pub struct Files {
    ed: RefCell<Option<Rc<Ed>>>,
    /// The file the last save or open went through the picker to, which the
    /// next ⌘S writes again.
    handle: RefCell<Option<JsValue>>,
    /// What waits to be kept, and whether a timer will keep it.
    pending: RefCell<Option<String>>,
    scheduled: Cell<bool>,
    keep: RefCell<Option<Closure<dyn Fn()>>>,
}

impl Files {
    /// The document the page kept, if it kept one and the address does not
    /// ask for the demo.
    pub fn kept() -> Option<Doc> {
        let search = web_sys::window()?.location().search().ok()?;
        if search
            .trim_start_matches('?')
            .split('&')
            .any(|p| p == "demo")
        {
            return None;
        }
        let stored = storage()?.get_item(KEPT).ok()??;
        let (name, source) = stored.split_once('\n')?;
        let mut doc = Doc::from_source(source.to_owned(), format_of(name)?).ok()?;
        if name != UNTITLED {
            doc.path = PathBuf::from(name);
        }
        Some(doc)
    }

    /// The editor, once there is one.
    pub fn attach(self: &Rc<Self>, ed: Rc<Ed>) {
        *self.ed.borrow_mut() = Some(ed);
        let this = self.clone();
        *self.keep.borrow_mut() = Some(Closure::new(move || {
            this.scheduled.set(false);
            if let (Some(kept), Some(storage)) = (this.pending.take(), storage()) {
                let _ = storage.set_item(KEPT, &kept);
            }
        }));
    }

    fn ed(&self) -> Option<Rc<Ed>> {
        self.ed.borrow().clone()
    }

    /// The editor has repainted `doc`: keep it, after a pause.
    pub fn remember(&self, doc: &Doc) {
        let kept = format!("{}\n{}", name_of(doc), doc.source);
        if self.pending.borrow().as_ref() == Some(&kept) {
            return;
        }
        *self.pending.borrow_mut() = Some(kept);
        if self.scheduled.get() {
            return;
        }
        // Before `attach` there is no timer, and the first change after it
        // schedules one.
        if let (Some(window), Some(keep)) = (web_sys::window(), self.keep.borrow().as_ref()) {
            let set = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                keep.as_ref().unchecked_ref(),
                500,
            );
            self.scheduled.set(set.is_ok());
        }
    }

    /// ⌘S, or with `ask` ⇧⌘S.
    pub fn save(self: &Rc<Self>, ask: bool) {
        let Some(ed) = self.ed() else { return };
        let this = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let (source, name) = ed.run(|e| {
                let d = e.doc();
                (d.source.clone(), name_of(&d))
            });
            let saved = match picker("showSaveFilePicker") {
                Some(pick) => this.save_picked(&pick, ask, &name, &source).await,
                None => download(&name, &source).then(|| name.clone()),
            };
            let Some(name) = saved else { return };
            ed.run(|e| {
                {
                    let mut d = e.doc();
                    // Clean only if nothing was typed while the file was written.
                    if d.source == source {
                        d.dirty = false;
                    }
                    d.path = PathBuf::from(name);
                }
                e.refresh();
            });
        });
    }

    /// Write `source` through the save picker — to the last file picked,
    /// unless `ask` — answering the file's name, or `None` if the person
    /// cancelled or the write failed.
    async fn save_picked(
        &self,
        pick: &Function,
        ask: bool,
        name: &str,
        source: &str,
    ) -> Option<String> {
        let last = (!ask).then(|| self.handle.borrow().clone()).flatten();
        let handle = match last {
            Some(h) => h,
            None => {
                let opts = Object::new();
                set(&opts, "suggestedName", &name.into());
                set(&opts, "types", &types());
                await_call(pick, &web_sys::window()?.into(), &[opts.into()]).await?
            }
        };
        let writable = await_method(&handle, "createWritable", &[]).await?;
        await_method(&writable, "write", &[source.into()]).await?;
        await_method(&writable, "close", &[]).await?;
        let name = Reflect::get(&handle, &"name".into()).ok()?.as_string()?;
        *self.handle.borrow_mut() = Some(handle);
        Some(name)
    }

    /// ⌘O: a document the person picks, in place of this one.
    pub fn open(self: &Rc<Self>) {
        let this = self.clone();
        match picker("showOpenFilePicker") {
            Some(pick) => wasm_bindgen_futures::spawn_local(async move {
                let opts = Object::new();
                set(&opts, "types", &types());
                let Some(window) = web_sys::window() else {
                    return;
                };
                let Some(handles) = await_call(&pick, &window.into(), &[opts.into()]).await else {
                    return;
                };
                let Some(handle) = Array::from(&handles).get(0).dyn_into::<JsValue>().ok() else {
                    return;
                };
                let Some(file) = await_method(&handle, "getFile", &[]).await else {
                    return;
                };
                if this.open_now(file.unchecked_into()).await {
                    *this.handle.borrow_mut() = Some(handle);
                }
            }),
            None => this.choose(),
        }
    }

    /// Open a file that came without a handle — dropped, or chosen through
    /// an `<input>` — so the next ⌘S asks where to write it.
    pub fn open_file(self: &Rc<Self>, file: web_sys::File) {
        let this = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if this.open_now(file).await {
                *this.handle.borrow_mut() = None;
            }
        });
    }

    async fn open_now(&self, file: web_sys::File) -> bool {
        let name = file.name();
        let Some(format) = format_of(&name) else {
            return false;
        };
        let Some(text) = JsFuture::from(file.text())
            .await
            .ok()
            .and_then(|t| t.as_string())
        else {
            return false;
        };
        let Ok(mut doc) = Doc::from_source(text, format) else {
            return false;
        };
        doc.path = PathBuf::from(name);
        doc.dirty = false;
        let Some(ed) = self.ed() else { return false };
        ed.run(|e| e.install(doc));
        true
    }

    /// The browser's own file chooser, where there is no open picker.
    fn choose(self: &Rc<Self>) {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let Ok(input) = doc.create_element("input") else {
            return;
        };
        let input: web_sys::HtmlInputElement = input.unchecked_into();
        input.set_type("file");
        let accept: Vec<String> = FORMATS.iter().map(|(e, _)| format!(".{e}")).collect();
        input.set_accept(&accept.join(","));
        let this = self.clone();
        let chosen = input.clone();
        let changed = Closure::once_into_js(move || {
            if let Some(file) = chosen.files().and_then(|f| f.get(0)) {
                this.open_file(file);
            }
        });
        input.set_onchange(Some(changed.unchecked_ref()));
        input.click();
    }
}

/// What the page calls the document: its file's name, or a new one's.
fn name_of(doc: &Doc) -> String {
    doc.path
        .file_name()
        .and_then(|n| n.to_str())
        .map_or_else(|| UNTITLED.to_owned(), str::to_owned)
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

/// A picker the browser has, by name.
fn picker(name: &str) -> Option<Function> {
    Reflect::get(&web_sys::window()?.into(), &name.into())
        .ok()?
        .dyn_into()
        .ok()
}

/// The pickers' file types: the formats leaf opens.
fn types() -> JsValue {
    let accept = Object::new();
    let exts: Array = FORMATS
        .iter()
        .map(|(e, _)| JsValue::from(format!(".{e}")))
        .collect();
    set(&accept, "text/plain", &exts);
    let kind = Object::new();
    set(&kind, "description", &"Documents".into());
    set(&kind, "accept", &accept);
    Array::of1(&kind).into()
}

fn set(o: &Object, key: &str, value: &JsValue) {
    let _ = Reflect::set(o, &key.into(), value);
}

async fn await_call(f: &Function, this: &JsValue, args: &[JsValue]) -> Option<JsValue> {
    let promise = f.apply(this, &args.iter().collect()).ok()?;
    JsFuture::from(js_sys::Promise::from(promise)).await.ok()
}

async fn await_method(o: &JsValue, name: &str, args: &[JsValue]) -> Option<JsValue> {
    let f: Function = Reflect::get(o, &name.into()).ok()?.dyn_into().ok()?;
    await_call(&f, o, args).await
}

/// Download `source` as a file named `name`; `false` if the page could not.
fn download(name: &str, source: &str) -> bool {
    let go = || -> Option<()> {
        let doc = web_sys::window()?.document()?;
        let parts = Array::of1(&source.into());
        let opts = web_sys::BlobPropertyBag::new();
        opts.set_type("text/plain;charset=utf-8");
        let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts).ok()?;
        let url = web_sys::Url::create_object_url_with_blob(&blob).ok()?;
        let a: web_sys::HtmlAnchorElement = doc.create_element("a").ok()?.unchecked_into();
        a.set_href(&url);
        a.set_download(name);
        a.click();
        let _ = web_sys::Url::revoke_object_url(&url);
        Some(())
    };
    go().is_some()
}
