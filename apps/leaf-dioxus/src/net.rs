//! A window's fetches: blitz-net's, but for the pictures the pages are
//! drawn from, which are `data:` URLs, loaded one at a time in the order
//! they were asked for.
//!
//! An `img` given a new `src` keeps drawing its picture until the new one
//! is loaded, which is what keeps a piece on show while a keystroke's
//! picture of it is decoded (`pages.rs`). Blitz puts a loaded picture on
//! every node that asked for its URL, whatever the node's `src` is by then:
//! loaded side by side, as blitz-net loads them, an older picture that
//! takes longer would land last, and a piece would show a keystroke
//! behind until the next. Loaded in order, the last asked for lands last.

use std::sync::{Arc, Mutex, mpsc};

use base64::Engine;
use blitz_traits::net::{Bytes, NetHandler, NetProvider, NetWaker, Request};

type Job = (usize, String, Bytes, Box<dyn NetHandler>);

pub struct Net {
    inner: Arc<blitz_net::Provider>,
    data: Mutex<mpsc::Sender<Job>>,
}

impl Net {
    /// The provider, and blitz-net's own under it, which a window's root
    /// context holds.
    pub fn new(waker: Arc<dyn NetWaker>) -> (Arc<Self>, Arc<blitz_net::Provider>) {
        let inner = Arc::new(blitz_net::Provider::new(Some(Arc::clone(&waker))));
        let (tx, rx) = mpsc::channel::<Job>();
        // Ends with the window, when its provider, the sender, is dropped.
        std::thread::Builder::new()
            .name("leaf-pictures".into())
            .spawn(move || {
                for (doc, url, bytes, handler) in rx {
                    handler.bytes(url, bytes);
                    waker.wake(doc);
                }
            })
            .expect("a thread to load pictures on");
        let net = Arc::new(Self {
            inner: Arc::clone(&inner),
            data: Mutex::new(tx),
        });
        (net, inner)
    }
}

impl NetProvider for Net {
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        match base64_data(&request) {
            Some(bytes) => {
                let url = request.url.to_string();
                let sent = self
                    .data
                    .lock()
                    .map(|tx| tx.send((doc_id, url, bytes, handler)));
                if !matches!(sent, Ok(Ok(()))) {
                    leaf_dioxus::trace(|| "net: the picture loader is gone".to_string());
                }
            }
            None => self.inner.fetch(doc_id, request, handler),
        }
    }
}

/// A `data:` URL's bytes, if it is one in base64 with nothing to abort it:
/// anything else is blitz-net's.
fn base64_data(request: &Request) -> Option<Bytes> {
    if request.url.scheme() != "data" || request.signal.is_some() {
        return None;
    }
    let (meta, body) = request
        .url
        .as_str()
        .strip_prefix("data:")?
        .split_once(',')?;
    if !meta.ends_with(";base64") {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    Some(Bytes::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pictures are loaded in the order asked for, however long each takes.
    #[test]
    fn data_urls_load_in_order() {
        struct Handler(usize, Arc<Mutex<Vec<usize>>>);
        impl NetHandler for Handler {
            fn bytes(self: Box<Self>, _: String, bytes: Bytes) {
                // The first, the biggest, takes longest.
                if self.0 == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                assert_eq!(&bytes[..], b"<svg/>");
                self.1.lock().unwrap().push(self.0);
            }
        }
        let woken = Arc::new(Mutex::new(0));
        let w = Arc::clone(&woken);
        let (net, _) = Net::new(Arc::new(move |_| *w.lock().unwrap() += 1));
        let loaded = Arc::new(Mutex::new(Vec::new()));
        let url = format!(
            "data:image/svg+xml;base64,{}",
            base64::engine::general_purpose::STANDARD.encode("<svg/>")
        );
        for i in 0..4 {
            let url = blitz_traits::net::Url::parse(&url).unwrap();
            net.fetch(
                0,
                Request::get(url),
                Box::new(Handler(i, Arc::clone(&loaded))),
            );
        }
        let start = std::time::Instant::now();
        while *woken.lock().unwrap() < 4 && start.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(*loaded.lock().unwrap(), [0, 1, 2, 3]);
    }
}
