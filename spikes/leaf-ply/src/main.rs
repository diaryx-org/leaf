//! Spike: leaf-core's editor drawn and driven through Ply.
//!
//! The host here is the `plyx init` skeleton with the editor dropped in, plus
//! the two things a host owns: the document's file and the input stream.
//!
//! ```sh
//! cargo run -- ../../sample.md
//! cargo run -- /tmp/test.md --script tour/tour.txt  # scripted run, see `Step`
//! ```

mod editor;
mod fonts;
mod layout;

use std::path::PathBuf;

use leaf_core::Doc;
use macroquad::input::utils::{register_input_subscriber, repeat_all_miniquad_input};
use macroquad::miniquad::{self, EventHandler, KeyMods};
use ply_engine::prelude::*;

use editor::{Editor, Ev, Mods, Outcome};

fn window_conf() -> macroquad::conf::Conf {
    macroquad::conf::Conf {
        miniquad_conf: miniquad::conf::Conf {
            window_title: "leaf · Ply spike".to_owned(),
            window_width: 1000,
            window_height: 760,
            // Logical = physical pixels, so miniquad's raw mouse coordinates
            // and Ply's layout agree without a scale factor in between.
            high_dpi: false,
            sample_count: 4,
            ..Default::default()
        },
        draw_call_vertex_capacity: 200_000,
        draw_call_index_capacity: 200_000,
        ..Default::default()
    }
}

/// Collects this frame's input in arrival order — see [`Ev`].
#[derive(Default)]
struct Collector {
    events: Vec<Ev>,
}

fn mods(m: KeyMods) -> Mods {
    Mods {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        logo: m.logo,
    }
}

impl EventHandler for Collector {
    fn update(&mut self) {}
    fn draw(&mut self) {}
    fn char_event(&mut self, ch: char, m: KeyMods, _repeat: bool) {
        self.events.push(Ev::Char(ch, mods(m)));
    }
    fn key_down_event(&mut self, k: KeyCode, m: KeyMods, _repeat: bool) {
        self.events.push(Ev::Key(k, mods(m)));
    }
    fn mouse_button_down_event(&mut self, b: MouseButton, x: f32, y: f32) {
        if b == MouseButton::Left {
            let shift = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
            self.events.push(Ev::MouseDown(
                x,
                y,
                Mods {
                    shift,
                    ..Mods::default()
                },
            ));
        }
    }
    fn mouse_button_up_event(&mut self, b: MouseButton, _x: f32, _y: f32) {
        if b == MouseButton::Left {
            self.events.push(Ev::MouseUp);
        }
    }
    fn mouse_motion_event(&mut self, x: f32, y: f32) {
        self.events.push(Ev::MouseMove(x, y));
    }
    fn mouse_wheel_event(&mut self, _x: f32, y: f32) {
        self.events.push(Ev::Wheel(y));
    }
}

/// One step of a `--script`, run one per frame. Lines:
///
/// ```text
/// type Hello, world        # each character as a char event
/// key Left shift           # a key, with any of: shift ctrl alt
/// click 300 120 [shift]    # press + release at window pixels
/// drag 300 120 500 120     # press, move, release
/// shot out.png             # screenshot after this frame
/// dump                     # print source, caret, selection
/// check                    # compare our run widths with Ply's
/// stat                     # caret, line count, last layout and frame time
/// quit
/// ```
enum Step {
    Events(Vec<Ev>),
    Shot(String),
    Dump,
    Check,
    Stat,
    Quit,
}

fn parse_script(text: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let words: Vec<&str> = rest.split_whitespace().collect();
        let flag = |f: &str| words.contains(&f);
        let m = Mods {
            shift: flag("shift"),
            ctrl: flag("ctrl"),
            alt: flag("alt"),
            logo: false,
        };
        let num = |i: usize| words[i].parse::<f32>().expect("number");
        steps.push(match cmd {
            "type" => Step::Events(rest.chars().map(|c| Ev::Char(c, Mods::default())).collect()),
            "key" => Step::Events(vec![Ev::Key(key_named(words[0]), m)]),
            "click" => Step::Events(vec![Ev::MouseDown(num(0), num(1), m), Ev::MouseUp]),
            "drag" => Step::Events(vec![
                Ev::MouseDown(num(0), num(1), Mods::default()),
                Ev::MouseMove(num(2), num(3)),
                Ev::MouseUp,
            ]),
            "shot" => Step::Shot(rest.trim().to_string()),
            "dump" => Step::Dump,
            "check" => Step::Check,
            "stat" => Step::Stat,
            "quit" => Step::Quit,
            other => panic!("unknown script step {other:?}"),
        });
    }
    steps
}

fn key_named(name: &str) -> KeyCode {
    match name {
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "Enter" => KeyCode::Enter,
        "Backspace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "Tab" => KeyCode::Tab,
        "B" => KeyCode::B,
        "I" => KeyCode::I,
        "E" => KeyCode::E,
        "Z" => KeyCode::Z,
        "A" => KeyCode::A,
        "Key1" => KeyCode::Key1,
        "Key2" => KeyCode::Key2,
        other => panic!("unknown key {other:?}"),
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args
        .first()
        .map(PathBuf::from)
        .expect("usage: leaf-ply-spike <file.md> [--script steps.txt]");
    let script = args
        .iter()
        .position(|a| a == "--script")
        .map(|i| parse_script(&std::fs::read_to_string(&args[i + 1]).expect("read script")));

    let fonts = fonts::Fonts::load().await;
    let mut ply = Ply::new(fonts.asset(fonts::Face::Regular)).await;
    let doc = Doc::open(path).expect("open document");
    let mut editor = Editor::new(doc, fonts);

    let subscriber = register_input_subscriber();
    let mut collector = Collector::default();
    let mut steps = script.map(|s| s.into_iter());
    let mut pending_shot: Option<String> = None;
    let mut frame = 0u64;

    loop {
        frame += 1;
        let now = get_time();
        repeat_all_miniquad_input(&mut collector, subscriber);
        let mut events = std::mem::take(&mut collector.events);

        // A scripted run takes over input after two frames, so the first
        // layout (and Ply's bounding box for the page) exist when it clicks.
        if let Some(it) = steps.as_mut().filter(|_| frame > 2) {
            events.clear();
            match it.next() {
                Some(Step::Events(evs)) => events = evs,
                Some(Step::Shot(p)) => pending_shot = Some(p),
                Some(Step::Dump) => dump(&editor),
                Some(Step::Check) => check(&editor),
                Some(Step::Stat) => println!(
                    "stat: caret {} · {} lines · {} bytes · core map {:.1} ms · spike wrap {:.1} ms · last frame {:.1} ms",
                    editor.doc.caret,
                    editor.lines().len(),
                    editor.doc.source.len(),
                    editor.last_core_ms,
                    editor.last_layout_ms,
                    get_frame_time() * 1000.0,
                ),
                Some(Step::Quit) | None => break,
            }
        }

        if editor.handle(&events, now) == Outcome::Quit {
            break;
        }

        editor.fonts().keep_loaded().await;
        clear_background(WHITE);
        let mut ui = ply.begin();
        editor.ui(&mut ui, now);
        ui.show(|_| {}).await;

        if let Some(p) = pending_shot.take() {
            if let Some(dir) = std::path::Path::new(&p).parent() {
                std::fs::create_dir_all(dir).ok();
            }
            get_screen_data().export_png(&p);
            println!("shot {p}");
        }
        next_frame().await;
    }
}

fn dump(editor: &Editor) {
    let doc = &editor.doc;
    println!(
        "--- source ---\n{}--- caret {} selection {:?} view {:?}",
        doc.source,
        doc.caret,
        doc.selection(),
        doc.view
    );
}

/// Does the x we computed for every run (a sum of per-character advances) match
/// the width Ply itself measures for the run's text element? If it does, the
/// caret and hit-testing agree with what is painted.
fn check(editor: &Editor) {
    let mut worst: f32 = 0.0;
    let mut runs = 0;
    for l in editor.lines() {
        for r in &l.runs {
            let ply_w = editor.fonts().run_width(r.face, r.size, &r.text);
            worst = worst.max((ply_w - r.width).abs());
            runs += 1;
        }
    }
    println!("check: {runs} runs, worst |ply width − summed advances| = {worst:.4}px");
}
