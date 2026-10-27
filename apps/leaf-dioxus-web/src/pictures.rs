//! Pictures, for a page that has no files: the browser's [`Host::picture`].
//!
//! The editor reads a picture's file from disk on the Mac; a browser has no
//! disk, so the page loads each picture itself and hands Blitz a `data:` URI
//! of it with its natural size:
//!
//! - a `data:` URI, as it is;
//! - a file dropped on the page, kept in memory under the name it was
//!   inserted with ([`Pictures::keep`]);
//! - anything else, fetched: a relative path against the page, so a picture
//!   served beside it draws, or a web address — which draws only when its
//!   server allows another site to read it (CORS), since a canvas cannot show
//!   what the page may not read. One that does not load is the editor's chip
//!   with its alt text.
//!
//! A fetch answers `None` until it lands, then repaints the editor.
//!
//! [`Host::picture`]: leaf_dioxus::Host::picture

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use base64::Engine;
use leaf_dioxus::{Picture, picture_size};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

enum Load {
    Pending,
    Ready(Picture),
    Failed,
}

#[derive(Default)]
pub struct Pictures {
    loads: RefCell<HashMap<String, Load>>,
    /// Files dropped on the page, by the name the document gives them.
    kept: RefCell<HashMap<String, Rc<[u8]>>>,
    /// The editor's repaint, once there is an editor.
    repaint: RefCell<Option<Rc<dyn Fn()>>>,
}

impl Pictures {
    /// Repaint the editor with `f` when a picture arrives.
    pub fn on_arrival(&self, f: impl Fn() + 'static) {
        *self.repaint.borrow_mut() = Some(Rc::new(f));
    }

    /// The picture `dest` names, if it is loaded; otherwise start loading it.
    pub fn get(self: &Rc<Self>, dest: &str) -> Option<Picture> {
        if let Some(load) = self.loads.borrow().get(dest) {
            return match load {
                Load::Ready(p) => Some(p.clone()),
                Load::Pending | Load::Failed => None,
            };
        }
        let at_hand = if dest.starts_with("data:") {
            Some(data_bytes(dest))
        } else {
            self.kept.borrow().get(dest).map(|b| Some(b.to_vec()))
        };
        match at_hand {
            Some(bytes) => {
                let load = bytes
                    .as_deref()
                    .and_then(ready)
                    .map_or(Load::Failed, Load::Ready);
                let answer = match &load {
                    Load::Ready(p) => Some(p.clone()),
                    _ => None,
                };
                self.loads.borrow_mut().insert(dest.to_owned(), load);
                answer
            }
            None => {
                self.loads
                    .borrow_mut()
                    .insert(dest.to_owned(), Load::Pending);
                let this = self.clone();
                let dest = dest.to_owned();
                wasm_bindgen_futures::spawn_local(async move {
                    let load = fetch(&dest)
                        .await
                        .as_deref()
                        .and_then(ready)
                        .map_or(Load::Failed, Load::Ready);
                    this.loads.borrow_mut().insert(dest, load);
                    let repaint = this.repaint.borrow().clone();
                    if let Some(repaint) = repaint {
                        repaint();
                    }
                });
                None
            }
        }
    }

    /// Keep a dropped file's bytes under `name`, which the document will
    /// name it by: with what a link's destination cannot hold bare (a space,
    /// a bracket) made a hyphen, and a number added to a name already kept.
    /// Answers the name kept.
    pub fn keep(&self, name: &str, bytes: Vec<u8>) -> String {
        let name: String = name
            .chars()
            .map(|c| match c {
                c if c.is_alphanumeric() || "._-~".contains(c) => c,
                _ => '-',
            })
            .collect();
        let mut kept = self.kept.borrow_mut();
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
            _ => (name.to_owned(), String::new()),
        };
        let mut name = format!("{stem}{ext}");
        let mut n = 1;
        while kept.contains_key(&name) || self.loads.borrow().contains_key(&name) {
            n += 1;
            name = format!("{stem}-{n}{ext}");
        }
        kept.insert(name.clone(), bytes.into());
        name
    }
}

/// The picture `bytes` hold, as a `data:` URI Blitz decodes, and its size.
fn ready(bytes: &[u8]) -> Option<Picture> {
    let size = picture_size(bytes)?;
    let mime = match image::guess_format(bytes) {
        Ok(f) => f.to_mime_type(),
        Err(_) => "image/svg+xml",
    };
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(Picture {
        url: format!("data:{mime};base64,{data}"),
        size,
    })
}

/// A `data:` URI's bytes: base64 or percent-encoded. `None` for one that
/// does not decode.
fn data_bytes(uri: &str) -> Option<Vec<u8>> {
    let (head, body) = uri.strip_prefix("data:")?.split_once(',')?;
    if head.ends_with(";base64") {
        let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::STANDARD.decode(body).ok()
    } else {
        js_sys::decode_uri_component(body)
            .ok()
            .map(|s| String::from(s).into_bytes())
    }
}

/// Fetch `dest`, relative to the page.
async fn fetch(dest: &str) -> Option<Vec<u8>> {
    let window = web_sys::window()?;
    let response = JsFuture::from(window.fetch_with_str(dest)).await.ok()?;
    let response: web_sys::Response = response.dyn_into().ok()?;
    if !response.ok() {
        return None;
    }
    let buffer = JsFuture::from(response.array_buffer().ok()?).await.ok()?;
    Some(js_sys::Uint8Array::new(&buffer).to_vec())
}
