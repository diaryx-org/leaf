---
title: Tests leave their fixtures in the temp directory
description: leaf-core's unit tests write every fixture to the shared temp directory and remove almost none, so each run leaves about 830 files behind and two runs at once write to the same paths
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: done
created: 2026-10-27
updated: 2026-10-27
---
# Tests leave their fixtures in the temp directory

**Done, 2026-10-27**, by `test(core): each test's fixtures live in a temp
directory of their own, removed when it ends`. `doc_in`, `missing_path` and
`temp_path` take their paths from `fixture_path`, which makes a
`tempfile::TempDir` for each and hands it to the calling test's thread, so
the directory goes when the test does, pass or fail. libtest runs every test
on a thread of its own, `--test-threads=1` included. The helpers still
return a `Doc` and a `PathBuf`, so no test changed. `TMPDIR=$(mktemp -d)
cargo test -p leaf-core` leaves that directory empty, and
`a_test_takes_its_fixtures_with_it` holds it.

**Where.** `crates/leaf-core/src/doc/tests/mod.rs`, `doc_in`, which every
`doc_with` and `wysiwyg_doc` goes through; and `missing_path` and
`temp_path` in `crates/leaf-core/src/doc/tests/file.rs`.

**What.** `doc_in` writes its body to
`std::env::temp_dir()/leaf_test_{name}_{seq}.md`, opens it, and never
removes it. The suite's only `remove_file` calls are ten in `file.rs`, a
few of them clearing a path before a test rather than after. So every run of the suite adds to the temp
directory, and nothing ever removes the files:

```console
$ T=$(mktemp -d); TMPDIR=$T cargo test -p leaf-core --lib
test result: ok. 883 passed; 0 failed; ...
$ ls $T | wc -l
830
$ TMPDIR=$T cargo test -p leaf-core --lib; ls $T | wc -l
1368
```

On hbox, where `TMPDIR` is unset, `/tmp` held 1,561 of these files on
2026-10-27. Besides the clutter, they get in the way of tools that read
the directory. `prov check` run in `/tmp` stops with "ambiguous workspace
root", naming the `leaf_test_*.md` files.

The `seq` counter that `doc_in`'s comment relies on to keep two tests apart
only counts within one process. It starts at 0 in every run, and the
second run above reused about 290 of the first run's names. So two suites
running at once, from two worktrees or two agents on one machine, write
the same path. One can overwrite the other's fixture, or a fixture a test
has just saved, between the write and the `Doc::open`, and the test then
reads the wrong body.

**How.** Give each test process a directory of its own, named with
`std::process::id()` as `crates/leaf-dioxus/src/host.rs` does, so two runs
never share a path. Then make sure each fixture is removed when its test
ends, for example with a guard that the helpers return alongside the
`Doc`, or with the `tempfile` crate as a dev-dependency. `missing_path`
and `temp_path` get the same treatment.

**Done when.** `TMPDIR=$(mktemp -d) cargo test -p leaf-core` leaves that
directory empty, or leaves only one directory per run that the run itself
removes, and no fixture path contains only a test's name and an in-process
counter.
