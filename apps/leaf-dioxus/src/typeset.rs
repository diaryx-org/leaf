//! Where a document is set: in the app's process, in one of twig-export's
//! own templates; in a typesetter of its own, in any other.
//!
//! A template is Typst, and Typst has no limit on how long it runs or how
//! much it takes: a `while true {}`, or a layout that never settles, would
//! hold the app, and a fault in Typst itself would take it down. So a
//! template not twig-export's is set by the app run again as a typesetter
//! (`Leaf --typeset`), kept running between documents so that Typst's
//! memory of the last setting makes the next quick, and set with a limit
//! on its memory ([`MEMORY`]) and on how long one setting may take
//! ([`TIME`]). One that goes over is ended, and the view says so; the next
//! setting starts another.
//!
//! The typesetter reads a request a line, as JSON, and answers each with a
//! line: the pages and their glyphs, a PDF, or why not.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use leaf_core::Format;
use serde_json::{Value, json};
use twig_export::pdf::{Glyph, Layout, Packages, Page, Typesetter};

use crate::pdf::{Sheet, Type};
use crate::settings::Template;

/// The most one setting may take before its typesetter is ended.
pub const TIME: Duration = Duration::from_secs(30);

/// The most memory a typesetter may have, in bytes.
pub const MEMORY: u64 = 2 << 30;

/// A document laid out, by this process or a typesetter: each page's size
/// and glyphs, and its pieces, twig-export's tiles, each with its picture
/// unless the window has that picture already.
pub struct Laid {
    pub pages: Vec<Page>,
    pub tiles: Vec<Tiled>,
}

/// A piece of a page as laid out: where it is on its page, in points, what
/// it draws as a key, and its SVG if the window asking has none of that key.
#[derive(Clone, Debug, PartialEq)]
pub struct Tiled {
    pub page: usize,
    pub top: f64,
    pub height: f64,
    pub width: f64,
    pub key: u128,
    pub svg: Option<String>,
}

/// `layout` as pages and pieces, a picture made for each piece whose key is
/// not `known`, once.
fn laid(layout: &Layout, known: &HashSet<u128>) -> Laid {
    let mut made = HashSet::new();
    let tiles = layout
        .tiles()
        .iter()
        .map(|t| Tiled {
            page: t.page,
            top: t.top,
            height: t.height,
            width: t.width,
            key: t.key,
            svg: (!known.contains(&t.key) && made.insert(t.key)).then(|| layout.tile_svg(t)),
        })
        .collect();
    Laid {
        pages: layout.pages(),
        tiles,
    }
}

/// What a setting is asked of: a document, how it is set, and on what.
#[derive(Clone, Debug)]
pub struct Job {
    pub source: String,
    pub format: Format,
    pub path: Option<PathBuf>,
    pub ty: Type,
    pub sheet: Sheet,
}

/// `job` laid out, with a picture for each piece whose key is not `known`.
pub fn layout(job: &Job, known: &HashSet<u128>) -> Result<Laid, String> {
    if is_ours(&job.ty.template) {
        return here(job).map(|l| laid(&l, known));
    }
    let known: Vec<String> = known.iter().map(|k| format!("{k:x}")).collect();
    let answer = ask(json!({ "kind": "layout", "job": job_json(job), "known": known }))?;
    laid_of(&answer)
}

/// `job` as a PDF.
pub fn pdf(job: &Job) -> Result<Vec<u8>, String> {
    if is_ours(&job.ty.template) {
        return here(job)?.pdf().map_err(|e| format!("{e:#}"));
    }
    let answer = ask(json!({ "kind": "pdf", "job": job_json(job) }))?;
    base64::engine::general_purpose::STANDARD
        .decode(answer["pdf"].as_str().unwrap_or_default())
        .map_err(|e| e.to_string())
}

/// Check that the template `key` sets a sample in each of `layouts`, in a
/// typesetter.
pub fn check(key: &str, layouts: &[String]) -> Result<(), String> {
    ask(json!({ "kind": "check", "template": key, "layouts": layouts })).map(|_| ())
}

/// Whether a template is twig-export's own, and set in this process.
fn is_ours(t: &Template) -> bool {
    twig_export::templates::builtin(t.key()).is_some()
}

/// The typesetter's own: the folder of templates, and today, which many
/// templates date a document by.
fn typesetter() -> &'static Typesetter {
    static TYPESETTER: OnceLock<Typesetter> = OnceLock::new();
    TYPESETTER.get_or_init(|| {
        Typesetter::new()
            .with_packages(Packages::new([crate::templates::packages()]))
            .with_today(Some(today()))
    })
}

fn here(job: &Job) -> Result<Layout, String> {
    let tree = leaf_core::parse_tree(&job.source, job.format).map_err(|e| e.to_string())?;
    let cfg = crate::pdf::config(&job.ty, job.sheet);
    let words = twig_export::render_words(tree, &job.source, &cfg).map_err(|e| format!("{e:#}"))?;
    here_words(&words, root(job.path.as_deref()))
}

/// Words already rendered, laid out in this process.
pub(crate) fn here_words(words: &twig_export::Words, root: &Path) -> Result<Layout, String> {
    typesetter()
        .lay_out(words, root)
        .map_err(|e| format!("{e:#}"))
}

/// The folder a document at `path` finds its pictures in.
pub fn root(path: Option<&Path>) -> &Path {
    path.and_then(Path::parent)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Today's date, in UTC, as (year, month, day).
fn today() -> (i32, u8, u8) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Howard Hinnant's days-to-civil.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m, d)
}

// ── The app's side ──────────────────────────────────────────────────────────

/// The running typesetter: the child, what it is written to, and its
/// answers, a line each, read on a thread of their own.
struct Worker {
    child: Child,
    input: ChildStdin,
    answers: Receiver<String>,
}

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

/// Ask the typesetter, starting one if none runs; end it if it takes too
/// long or stops.
fn ask(request: Value) -> Result<Value, String> {
    let mut slot = WORKER.lock().map_err(|_| "the typesetter is lost")?;
    if slot.is_none() {
        *slot = Some(start()?);
    }
    let w = slot.as_mut().expect("a typesetter");
    let line = format!("{request}\n");
    let sent = w
        .input
        .write_all(line.as_bytes())
        .and_then(|_| w.input.flush());
    let answer = match sent {
        Ok(()) => w.answers.recv_timeout(TIME),
        Err(_) => Err(RecvTimeoutError::Disconnected),
    };
    let answer = match answer {
        Ok(a) => a,
        Err(e) => {
            let mut w = slot.take().expect("a typesetter");
            let _ = w.child.kill();
            let status = w.child.wait().ok();
            return Err(match e {
                RecvTimeoutError::Timeout => format!(
                    "The template took more than {} seconds to set the document, and was stopped.",
                    TIME.as_secs()
                ),
                RecvTimeoutError::Disconnected => format!(
                    "The template stopped the typesetter{}.",
                    status
                        .filter(|s| !s.success())
                        .map(|s| format!(" ({s}); it may have run out of memory"))
                        .unwrap_or_default()
                ),
            });
        }
    };
    let answer: Value = serde_json::from_str(&answer).map_err(|e| e.to_string())?;
    match answer.get("error").and_then(Value::as_str) {
        Some(e) => Err(e.to_owned()),
        None => Ok(answer),
    }
}

/// Start a typesetter: this program, with `--typeset`, its memory limited.
fn start() -> Result<Worker, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("--typeset")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setrlimit` is async-signal-safe, and nothing else runs
        // between the fork and the exec. The memory is the heap's (RLIMIT_DATA,
        // which counts Linux's anonymous mappings too), not the address
        // space's, which mapped fonts and thread stacks fill without using.
        unsafe {
            cmd.pre_exec(|| {
                let limit = libc::rlimit {
                    rlim_cur: MEMORY as libc::rlim_t,
                    rlim_max: MEMORY as libc::rlim_t,
                };
                if libc::setrlimit(libc::RLIMIT_DATA, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // A typesetter a template took down is no crash to report.
                let none = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                if libc::setrlimit(libc::RLIMIT_CORE, &none) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("starting the typesetter: {e}"))?;
    let input = child.stdin.take().ok_or("the typesetter has no input")?;
    let output = child.stdout.take().ok_or("the typesetter has no output")?;
    let (tx, answers) = channel();
    std::thread::Builder::new()
        .name("typesetter answers".into())
        .spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Worker {
        child,
        input,
        answers,
    })
}

// ── The typesetter's side ───────────────────────────────────────────────────

/// Be a typesetter: answer each request on stdin with a line on stdout,
/// until stdin closes.
pub fn serve() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(request) => answer(&request).unwrap_or_else(|e| json!({ "error": e })),
            Err(e) => json!({ "error": e.to_string() }),
        };
        if writeln!(stdout, "{answer}")
            .and_then(|_| stdout.flush())
            .is_err()
        {
            break;
        }
    }
}

fn answer(request: &Value) -> Result<Value, String> {
    match request["kind"].as_str() {
        Some("layout") => {
            let layout = here(&job_of(&request["job"])?)?;
            let known: HashSet<u128> = request["known"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|k| u128::from_str_radix(k.as_str()?, 16).ok())
                .collect();
            Ok(laid_json(&laid(&layout, &known)))
        }
        Some("pdf") => {
            let pdf = here(&job_of(&request["job"])?)?
                .pdf()
                .map_err(|e| format!("{e:#}"))?;
            Ok(json!({ "pdf": base64::engine::general_purpose::STANDARD.encode(pdf) }))
        }
        Some("check") => {
            let template = request["template"].as_str().unwrap_or_default();
            let layouts: Vec<String> = request["layouts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| l.as_str().map(str::to_owned))
                .collect();
            typesetter()
                .check(template, &layouts)
                .map_err(|e| format!("{e:#}"))?;
            Ok(json!({}))
        }
        other => Err(format!("no such request: {other:?}")),
    }
}

// ── On the wire ─────────────────────────────────────────────────────────────

fn job_json(job: &Job) -> Value {
    json!({
        "source": job.source,
        "format": format_name(job.format),
        "path": job.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "template": job.ty.template.key(),
        "size": job.ty.size,
        "dark": job.ty.dark,
        "sheet": match job.sheet {
            Sheet::Paper(p) => json!({ "paper": [p.width, p.height, p.margin] }),
            Sheet::Reflow { width } => json!({ "reflow": width }),
        },
    })
}

fn job_of(v: &Value) -> Result<Job, String> {
    let f = |k: &str| v[k].as_f64().unwrap_or_default() as f32;
    let sheet = &v["sheet"];
    let sheet = if let Some(p) = sheet["paper"].as_array() {
        let n = |i: usize| p.get(i).and_then(Value::as_f64).unwrap_or_default() as f32;
        Sheet::Paper(crate::pdf::Paper {
            width: n(0),
            height: n(1),
            margin: n(2),
        })
    } else {
        Sheet::Reflow {
            width: sheet["reflow"].as_f64().unwrap_or(480.0) as f32,
        }
    };
    Ok(Job {
        source: v["source"].as_str().unwrap_or_default().to_owned(),
        format: format_of(v["format"].as_str().unwrap_or_default())?,
        path: v["path"].as_str().map(PathBuf::from),
        ty: Type {
            template: Template::parse(v["template"].as_str().unwrap_or_default())
                .ok_or("no template named")?,
            size: f("size"),
            dark: v["dark"].as_bool().unwrap_or_default(),
        },
        sheet,
    })
}

const FORMATS: &[(Format, &str)] = &[
    (Format::Markdown, "markdown"),
    (Format::Djot, "djot"),
    (Format::Html, "html"),
    (Format::Asciidoc, "asciidoc"),
    (Format::Xml, "xml"),
];

fn format_name(f: Format) -> &'static str {
    FORMATS
        .iter()
        .find(|(g, _)| *g == f)
        .map_or("markdown", |(_, n)| n)
}

fn format_of(name: &str) -> Result<Format, String> {
    FORMATS
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(f, _)| *f)
        .ok_or_else(|| format!("no format named {name:?}"))
}

fn laid_json(l: &Laid) -> Value {
    let pages: Vec<Value> = l.pages.iter().map(page_json).collect();
    let tiles: Vec<Value> = l
        .tiles
        .iter()
        .map(|t| {
            json!({
                "page": t.page, "top": t.top, "height": t.height, "width": t.width,
                "key": format!("{:x}", t.key), "svg": t.svg,
            })
        })
        .collect();
    json!({ "pages": pages, "tiles": tiles })
}

fn laid_of(v: &Value) -> Result<Laid, String> {
    let pages = v["pages"]
        .as_array()
        .ok_or("the typesetter sent no pages")?;
    let tiles = v["tiles"]
        .as_array()
        .ok_or("the typesetter sent no pieces")?;
    let n = |t: &Value, k: &str| t[k].as_f64().unwrap_or_default();
    Ok(Laid {
        pages: pages.iter().map(page_of).collect(),
        tiles: tiles
            .iter()
            .map(|t| Tiled {
                page: t["page"].as_u64().unwrap_or_default() as usize,
                top: n(t, "top"),
                height: n(t, "height"),
                width: n(t, "width"),
                key: t["key"]
                    .as_str()
                    .and_then(|k| u128::from_str_radix(k, 16).ok())
                    .unwrap_or_default(),
                svg: t["svg"].as_str().map(str::to_owned),
            })
            .collect(),
    })
}

fn page_json(page: &Page) -> Value {
    let glyphs: Vec<Value> = page
        .glyphs
        .iter()
        .map(|g| {
            json!([
                g.x,
                g.baseline,
                g.width,
                g.ascent,
                g.descent,
                g.text,
                g.source.as_ref().map(|r| [r.start, r.end]),
            ])
        })
        .collect();
    json!({ "width": page.width, "height": page.height, "glyphs": glyphs })
}

fn page_of(v: &Value) -> Page {
    let glyphs = v["glyphs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| {
            let n = |i: usize| g[i].as_f64().unwrap_or_default();
            Glyph {
                x: n(0),
                baseline: n(1),
                width: n(2),
                ascent: n(3),
                descent: n(4),
                text: g[5].as_str().unwrap_or_default().to_owned(),
                source: g[6].as_array().and_then(|r| {
                    Some(r.first()?.as_u64()? as usize..r.get(1)?.as_u64()? as usize)
                }),
            }
        })
        .collect();
    Page {
        width: v["width"].as_f64().unwrap_or_default(),
        height: v["height"].as_f64().unwrap_or_default(),
        glyphs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_and_a_page_cross_the_wire_unchanged() {
        let job = Job {
            source: "# Hi\n".into(),
            format: Format::Djot,
            path: Some("/a/b.dj".into()),
            ty: Type {
                template: Template::parse("@adapted/memo:1.0.0").unwrap(),
                size: 12.0,
                dark: true,
            },
            sheet: Sheet::Reflow { width: 500.0 },
        };
        let back = job_of(&job_json(&job)).unwrap();
        assert_eq!(back.source, job.source);
        assert_eq!(back.format, job.format);
        assert_eq!(back.path, job.path);
        assert_eq!(back.ty, job.ty);
        assert_eq!(back.sheet, job.sheet);

        let page = Page {
            width: 612.0,
            height: 792.0,
            glyphs: vec![Glyph {
                x: 1.5,
                baseline: 20.0,
                width: 6.0,
                ascent: 9.0,
                descent: 3.0,
                text: "H".into(),
                source: Some(2..3),
            }],
        };
        assert_eq!(page_of(&page_json(&page)), page);
        let laid = Laid {
            pages: vec![page],
            tiles: vec![Tiled {
                page: 0,
                top: 10.0,
                height: 20.0,
                width: 612.0,
                key: u128::MAX - 7,
                svg: Some("<svg/>".into()),
            }],
        };
        let back = laid_of(&laid_json(&laid)).unwrap();
        assert_eq!((back.pages, back.tiles), (laid.pages, laid.tiles));
    }
}
