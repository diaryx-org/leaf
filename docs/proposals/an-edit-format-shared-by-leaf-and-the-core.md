---
title: An edit format shared by leaf and the core
part_of: '[Proposals](/docs/proposals/proposals.md)'
status: draft
created: 2026-10-22
---

# An edit format shared by leaf and the core

## Status

`draft`, 2026-10-22. Written by reading, with nothing built. Sources: leaf
at `b6a4eebd`, diaryx `origin/main` at `a3f10fce6`, historica at `2fd34e7`,
twig and fig as checked out in the org directory. The decisions under
[Decided](#decided) are Adam's, made in a conversation on 2026-10-22 before
this was written. Choices marked *(proposed)* are this document's, and are
open to change.

It answers decision 11 of Diaryx's
[The author's app runs in a browser](id:dxg6h4ncm/v1b592m): "Edits instead
need an edit format shared by the core and leaf's `Doc`, which leaf is to
design first."

## Open questions

1. **The new repository.** Its working name is `eg-walker-merge` (free on
   crates.io on 2026-10-22). Making the repository is Adam's.
2. **What names a live event** *(proposed: `(agent, seq)`)*. historica names
   an item by a revision's digest, because decision 0001 calls a stored
   change id an unverifiable claim. A live graph is never stored (decision
   7 below), so that objection does not apply to it, and `(agent, seq)` is
   16 bytes where a digest is 32. The crate is generic over the name, so
   each user picks. Is a different rule for live events acceptable?
3. **Where an agent id comes from** *(proposed: a random `u64` for each
   replica, for each open page)*. The editor and the worker are two agents
   on one device. A device id would also work, but two windows on one
   device would then share one.
4. **How often the editor sends** *(proposed: once per frame that changed
   the text)*. Per keystroke is more messages. A longer debounce makes a
   sync's merge see older text.
5. **How far back undo reaches** *(proposed: 200 steps, twig's cap now)*.
   The graph keeps every event that an undo step can still name, so the cap
   also limits the graph's memory.
6. **Markdown after a merge.** Two people who edit one bold run at once can
   merge into broken markup. leaf's mark-edge rule repairs only local typing.
   Should a merged text get the same repair, as a local event, or stay as
   merged?
7. **The other hosts.** leaf-ffi (the Swift apps) and leaf-wasm
   (`LeafEditor`) keep whole-text hosts. Do they take events too, and when?
8. **Live peers.** How events reach another device (through Diaryx Cloud,
   encrypted) is a later proposal. This one only keeps the format ready for
   it.

## Decided

Adam, 2026-10-22:

1. **Positions are UTF-8 byte offsets into the page's body**: the text the
   editor holds, which `read_body` returns. Frontmatter is not in it.
2. **An edit is a list of retain, insert and delete ops** over the whole
   text.
3. **Eg-walker events.** An edit is an event: ops, with a name and the names
   of its parents. Concurrent events merge by Eg-walker with Fugue's
   ordering, as historica merges revisions. The same format serves the page
   and the worker now, and live peers later.
4. **One merge crate, verified with Verus**, in a new public repository,
   used by leaf and historica. historica's walk and its proofs move into it.
5. **twig gets a major version first**: ops applied with one reparse, and no
   snapshot history under leaf.
6. **Undo is local only, and lives in the merge layer.** ⌘Z takes back only
   the person's own edits, rebased past the others. An undo is an event.
7. **The editor and the worker each hold a replica** of the open page. The
   graph is discarded when the page settles, and historica records a
   revision from the text, as now.
8. **A restore from history is one local, undoable edit**, not a new `Doc`.

## What exists

- **leaf.** A `Doc` is a twig `Editor` plus a byte-offset caret and
  selection. Every change to the text reaches twig through
  `edit_range(start, end, text)` (`doc/splice.rs`, `splice_exact`) or one of
  a few twig verbs, and every one passes `Doc::refresh`. twig returns a
  `Change { old, new }`, which is the edit's span before and after. twig's
  undo keeps up to 200 **whole-source snapshots**, each with a caret blob
  (`twig/src/ast/splicer.rs`, `MAX_UNDO`). `Doc::replace_source` reduces a
  new text to the one span that differs, and leaves the caret where it was
  (`doc/editing.rs`). `leaf-session` already has
  `Splice { range, text }`.
- **The Diaryx editor.** The page loads the body into `Doc::from_source`.
  It saves the **whole body** 600 ms after the last edit, through
  `Vault::save_body`, which writes the whole file again
  (`apps/diaryx/src/shell/mod.rs`, `fn Page`). Before it saves, it compares
  the file with its `base`, the body as last read or written. If they
  differ, it runs `cloud_merge_edit(base, ours, disk)`, a **line-based
  3-way merge** (`workspace/src/replica.rs`, `merge_edit`).
- **A sync's change to the open page** takes the same merge and then
  `Doc::replace_source`. The change is one undo step, so ⌘Z can take back
  another device's words.
- **A restore** writes the file, pops back, and mounts a new page with a
  new `Doc` (`shell/screens.rs`, `VersionView`). That is decision 11's "a
  whole new document".
- **historica** stores, for each revision and file, line operations
  (`Delete` or `Insert` at a line index into the parents' state). It
  computes them by diffing whole files when it records, not from the
  editor. It merges concurrent revisions with Eg-walker and Fugue
  (`src/merge.rs`, decision 0007). Its walk is proved in Verus to give one
  file for every causal order of one history (`src/merge/proof.rs`,
  decision 0076). It names an item `(revision digest, index)` and stores no
  change ids. The walk is batch: it builds its structure, reads the file
  out, and throws the structure away.
- **The worker protocol** (decision 10) is `Request` and `Reply`, deriving
  fig's `ToValue` and `FromValue` with an internal tag
  (`apps/diaryx/src/worker/mod.rs`). It has `Walk`, `Settle` and `Search`.
  Nothing yet reads, saves or announces a page. fig writes a struct as a
  map, an integer as `Int` (or `Uint` above `i64::MAX`), and a `String` as
  `Str`. It has no byte-string type.
- **Positions elsewhere.** Diaryx's `AnchoredRange` (remarks and
  highlights) counts bytes. historica-remark finds a quote again in each
  text and does not carry positions through edits.

## The proposal

### An edit is an event

```rust
/// One edit, as one replica made it.
pub struct Event {
    /// Who made it, and its place in that agent's own sequence.
    pub name: Name,
    /// The events it was made on top of: the replica's frontier at the time.
    /// Empty for an event made on the page as it was opened.
    pub parents: Vec<Name>,
    /// What it did, against the text at `parents`.
    pub ops: Vec<Op>,
}

pub struct Name {
    pub agent: u64,
    pub seq: u64,
}

pub enum Op {
    /// Keep the next `len` bytes.
    Retain { len: u64 },
    /// Write `text` here.
    Insert { text: String },
    /// Remove the next `len` bytes.
    Delete { len: u64 },
}
```

The ops read the text at the event's parents from its start. A retain at
the end is left out. Two ops of one kind are never adjacent, and an insert
comes before a delete at one position *(proposed)*, so one change has one
spelling. Every position falls on a `char` boundary of the text at the
parents. The crate refuses an event that breaks a rule, and so it never
splits a character.

An event holds any number of splices. A typed word, a paste, a Replace All
over twelve matches, and a restore are each one event.

### Positions

Bytes of the body, in UTF-8 (decision 1). That is the unit of `Doc`, of
twig, of `AnchoredRange`, and of the body's `String`, so no host converts
except at its own edge. The browser adapter converts the DOM's UTF-16
offsets, as leaf-wasm does now.

Positions count the body only. A change to the frontmatter, which flower
edits apart from the body, moves none of them.

### In leaf's `Doc`

The `Doc` keeps a replica beside its twig `Editor`: the event graph, its
frontier, and its own agent.

- **A local edit becomes an event in `refresh`.** That is the one funnel
  every change passes, and twig's `Change` there gives the span. leaf adds
  `Retain(old.start)`, `Delete(old.len())` and `Insert(new text)`. A gesture
  of several twig edits (a split list item that is renumbered, a mark-edge
  repair) is one event *(proposed)*: the ops of each edit are composed until
  the gesture returns.
- **Out:** `Doc::take_events() -> Vec<Event>` drains what the host has not
  yet sent.
- **In:** `Doc::apply(events) -> Result<(), Refused>` integrates events
  from another replica. The crate returns them as splices against the
  current text, and twig's new `apply` makes them one change with one
  reparse (see [twig](#twig)). The applied change is not an undo step, does
  not make the document dirty, and does not drop pending marks.
- **`replace_source` stays**, for a host that only has a whole new text.
  Its differing span becomes a local event.
- **`dirty`** keeps its meaning to the user: the text differs from what was
  last saved.

### The caret, the selection and highlights

A remote splice moves every position after it, as `carry_selection` does
now. A position at exactly the place of a remote insert stays **before**
it *(proposed)*. A person who types at the end of a line sees another
person's text arrive after the caret, not under it. A selection whose
text a remote delete takes collapses to the place of the delete.
Highlights move by the same rule.

### In the core

The worker holds a replica of each open page *(proposed messages)*:

```rust
#[derive(fig::ToValue, fig::FromValue)]
#[fig(tag = "ask")]
pub(crate) enum Request {
    // …
    /// Open `path` live: the body, and the worker's replica of it.
    Open { path: String },
    /// Events the editor made.
    Edit { path: String, events: Vec<Event> },
    /// The page is closing. The worker writes the text and drops the graph.
    Close { path: String },
    /// The body of `path` at an earlier revision, for a restore.
    Revision { path: String, at: String },
}

#[derive(fig::ToValue, fig::FromValue)]
#[fig(tag = "notice")]
pub(crate) enum Notice {
    /// Events the worker made, from a sync, for the editor.
    Changed { path: String, events: Vec<Event> },
}
```

`Notice` is new: a worker message with no request. Decision 10's
`postMessage` carries it, and natively it is a second channel.

- **Saving.** The editor sends events, not bodies. The worker applies them
  and writes the body through `save_body` on its own debounce. The editor's
  whole-body save and its `base` go.
- **A sync** that changes an open page does not write the page's file. The
  worker merges the new text into its live text with `merge_edit`, as the
  page does now. It then makes the difference one event of its own, and
  sends it as `Changed`. Events from the editor that cross it in flight are
  concurrent with it, and both replicas merge them to the same text.
- **A restore** of the open page asks `Revision` for the old body. The
  editor then calls `replace_source` with it. The restore is one local
  event and one undo step (decision 8). The page no longer pops back and
  remounts.
- **Settling.** `Close` writes the text and discards the graph. historica
  records a revision from the file, as now (decision 7). Its store does not
  change.
- **A refused event** means the replicas disagree on their history. The
  worker answers `Failed`, and the page falls back to decision 11's
  behaviour: it opens the page again as a new `Doc`.

### How it serializes

`Event`, `Name` and `Op` derive fig's `ToValue` and `FromValue` behind the
crate's `fig` feature *(proposed)*, so the crate itself depends on no
format. With fig's internal tag, one typed word is:

```text
{ name: { agent: 9182736455, seq: 41 },
  parents: [ { agent: 9182736455, seq: 40 } ],
  ops: [ { op: "retain", len: 1208 }, { op: "insert", text: "word" } ] }
```

The size is a few dozen bytes for each event, whatever the length of the
page. A `u64` above `i64::MAX` goes as fig's `Uint` and reads back exactly.
The text goes as a `Str`, so it must be UTF-8, which the body already is.

### Concurrent edits

Each replica applies its own events at once. An event from another replica
whose parents it already has goes straight onto the text if nothing is
concurrent with it. That is the common case, and Eg-walker makes it cost
nothing. When events are concurrent, the crate walks the graph back to the
last version both had, and integrates them by Fugue's rule. Fugue keeps
two concurrent runs of typing apart rather than interleaved. Every replica
that has the same events has the same text, whatever order they came in.
That is the theorem historica's proof already states for its walk.

Three kinds of concurrency use the one mechanism:

- **The editor and the worker on one device**, now. A sync's event crosses
  the editor's typing in flight.
- **Two devices, offline**, as now. historica merges their revisions by
  lines. The worker's live replica only sees the result, as a text, through
  `merge_edit`.
- **Two people, live**, later. Their replicas exchange events through a
  relay that cannot read them. A CRDT needs no server that transforms ops,
  and an end-to-end encrypted library cannot have one. That is why this
  proposal does not use OT with a central sequencer.

### Undo

Undo is local only (decision 6). leaf keeps its undo stack as **steps**.
A step is the names of the local events it took, with the caret before and
after. Coalescing of typing, undo groups and the 200-step cap keep their
rules from `doc/undo.rs`.

To undo a step, leaf asks the crate for one inverse event for each of the
step's events, newest first. The inverse of an event is stated **against
that event's own version**, and the event is its only parent: an insert
becomes a delete of the items it made, and a delete becomes an insert of
the text it removed. The inverse is then concurrent with everything made
since, local or remote, and the merge rebases it. Another device's words
stay. Text that another device already deleted stays deleted. Redo is the
inverse of the undo events, in the same way.

So undo needs no transform code of its own. It is correct when the merge is
correct, and the proof covers it.

### twig

twig's snapshot history cannot undo past a remote edit, and keeps up to 200
copies of the page. Decision 5 changes twig first, in a major version:

- **`Editor::apply(splices)`**: several splices, each against the text the
  one before it left, with one reparse and one bounding `Change`.
- **History becomes optional**, off for leaf. `undo`, `redo`,
  `coalesce_last_undo` and the caret blob are twig's only for a caller that
  asks for them. Other twig callers keep their behaviour if they ask.

### eg-walker-merge

The shared crate holds historica's walk, made generic:

- **over the item**: a line with its terminator for historica, a byte for
  leaf;
- **over the name**: `(revision digest, index)` for historica,
  `(agent, seq)` for live events;

and adds three things historica does not need:

- **incremental integration**: one event against a retained graph, without
  a replay of the whole history;
- **ops**: `Op`, `Event`, the canonical spelling, and splices out;
- **inverse events**, for undo.

historica's proofs move with the walk. New proof obligations *(proposed)*:

1. Incremental integration gives the same text as a batch walk of the same
   events. So a live replica agrees with what historica would merge.
2. With byte items, a merge never puts a position inside a UTF-8 scalar
   when every event's positions are on boundaries in its own version.
3. An inverse event applied with nothing after its target gives back the
   text before the target.
4. Ops to splices and back is the identity on canonical events.

historica keeps its behaviour and its store. Its conformance suite against
the Fugue reference must pass unchanged on the crate.

## Weighed against

- **A whole new document each time** (decision 11 as it stands). No new
  code, but the caret jumps, undo is lost, and a large page crosses the
  worker boundary on every sync.
- **One splice per edit.** Simplest, but a Replace All is many messages,
  and it composes no better than ops.
- **OT with the worker as sequencer.** Less code now. It cannot serve live
  peers in an encrypted library, because no party can transform. The format
  would change again.
- **A CRDT library** (yrs, loro, automerge). Proven, but a large dependency
  with its own wire format instead of fig values, metadata for each
  character, and a second merge algorithm beside historica's.
- **diamond-types.** The Eg-walker reference in Rust. Less code of ours, but
  not verified, and historica would still have its own walk.
- **Undo in twig**, as inverse ops transformed over each foreign op. It
  keeps undo in twig, but it adds a second transform with no proof.
- **The editor's replica only**, with the worker told on save. Fewer
  messages, but a sync between saves merges with older text.

## What it costs

- **The order.** twig's major version and the crate come before any of
  leaf or Diaryx changes. The crate needs a new repository.
- **The proof work.** historica's walk is about 4,800 lines and its proof
  about 3,800. Making them generic over the item and the name, and adding
  the incremental walk, is the largest part. Verus may make generic proofs
  harder than concrete ones; that is unmeasured.
- **Memory.** A replica keeps the events that undo can still name. A long
  session of typing makes thousands. Runs of typing compact into one event
  each, and the cap bounds the rest. Unmeasured.
- **Messages.** One a frame while typing, each a few dozen bytes. Small
  next to the whole body that a save sends now.
- **Two histories of one page while it is open**: the live graph, and
  historica's revisions. The live one is discarded at settle, so only
  historica's lasts.

## Tasks this would spawn

In order. A task in another repository is that repository's to write.

**eg-walker-merge** (new repository):

1. Move historica's walk and its proofs, generic over the item and the
   name. historica's conformance suite passes on it.
2. Incremental integration against a retained graph, with proof 1.
3. `Op`, `Event` and `Name`, the canonical spelling, splices out, the `fig`
   feature, with proof 4.
4. Byte items, with proof 2.
5. Inverse events, with proof 3.

**twig** (major version):

6. `Editor::apply` of several splices with one reparse.
7. History optional, and off where the caller asks.

**historica:**

8. Merge through eg-walker-merge, with the same bytes from every store.

**leaf:**

9. leaf-core: a replica in `Doc`. `refresh` records events,
   `take_events` and `apply` cross them, and the caret, the selection and
   highlights move with a remote change.
10. leaf-core: undo and redo as inverse events, on twig with no history.
    Steps, groups and coalescing keep their tests.
11. leaf-dioxus: the editor hands its events to the host and takes the
    host's in, through the host seam.
12. leaf-ffi and leaf-wasm, when their hosts want it (open question 7).

**diaryx:**

13. The worker protocol: `Open`, `Edit`, `Close`, `Revision`, and the
    `Notice` channel, native and in the browser.
14. The worker holds each open page live, writes it on its own debounce,
    and turns a sync's change into an event. The page's whole-body save,
    its `base`, and its `Arrived` merge go.
15. A restore of the open page becomes a local edit. Decision 11 in
    [The author's app runs in a browser](id:dxg6h4ncm/v1b592m) is
    updated to say so.
