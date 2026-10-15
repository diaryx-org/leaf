//! Moving a whole block up or down, and where a dragged block would land.

use super::*;

impl Doc {
    // ── moving a block ─────────────────────────────────────────────────────────

    /// The block a move picks up at `offset` — the same one
    /// [`select_block_at`](Self::select_block_at) selects (the deepest node
    /// that is neither inline nor a multi-block container), widened to the
    /// list item when it is the item's first block, because that is what
    /// twig's `move_block` moves: a bullet's text is the bullet, and dragging
    /// it takes the item and everything under it. A block later in an item's
    /// tail moves alone. `None` on a blank line, and past the source.
    fn movable_block_at(&mut self, offset: usize) -> Option<FlatNode> {
        let off = offset.min(self.source.len());
        let chain = self.editor.ancestors_at(off).ok()?;
        let block = chain
            .iter()
            .rev()
            .find(|m| !wysiwyg::is_inline_kind(&m.kind) && !is_block_container(&m.kind))?;
        let nodes = self.nodes();
        let block = nodes.get(block.node_id as usize)?.clone();
        let item = block
            .parent
            .and_then(|p| nodes.get(p.0 as usize))
            .filter(|p| matches!(p.kind, Kind::ListItem | Kind::TaskListItem))
            .filter(|p| p.first_child == Some(block.id));
        Some(item.cloned().unwrap_or(block))
    }

    /// The source range of the block a move would pick up at `offset` — for
    /// the outline of the block being carried, without moving the caret. The
    /// range [`select_block_at`](Self::select_block_at) would select, except
    /// that it is the whole item for a bullet's text, as
    /// [`move_block`](Self::move_block) is.
    pub fn block_range_at(&mut self, offset: usize) -> Option<Range<usize>> {
        self.movable_block_at(offset).map(|b| b.span)
    }

    /// Move the block at `from` to the boundary `to` — twig's `move_block`,
    /// with the undo plumbing every structural gesture has and the caret
    /// riding the block to its new place. `from` is any offset inside the
    /// block (the one [`block_range_at`](Self::block_range_at) finds); `to`
    /// is a position *between* blocks — a block's first byte lands before it,
    /// its last after it, a blank line is itself a boundary, and the source's
    /// length is the document's end. The block takes the line prefixes of
    /// the container the boundary is inside — a `> ` on the way into a quote,
    /// none on the way out — and the blank lines a person would have typed
    /// are written and removed; twig spells all of that.
    ///
    /// A move that would move nothing — `to` inside the block, or on the
    /// boundary it already sits on — is a quiet no-op rather than an error,
    /// since it is what a block dropped back where it was asks for. Any other
    /// refusal reaches the status line: a `to` interior to a fence or a table,
    /// or a format with no blocks a caret could name.
    ///
    /// In WYSIWYG the frontmatter is hidden, and a boundary above it is not
    /// one a person can see: `to` is raised to the first rendered offset.
    ///
    /// `true` when the document changed. A move twig accepts that rewrites
    /// nothing — a one-item list sent past a paragraph is still a one-item
    /// list past that paragraph — is taken back rather than left as an undo
    /// step with no difference in it, and is `false` too.
    pub fn move_block(&mut self, from: usize, to: usize) -> bool {
        if self.read_only || self.refuse_unsupported("move block", Gesture::MoveBlock) {
            return false;
        }
        let len = self.source.len();
        let from = from.min(len);
        let to = to.clamp(self.caret_floor().min(len), len);
        let Some(block) = self.movable_block_at(from) else {
            self.status = Some("move block: no block here".into());
            return false;
        };
        // Where the caret stands in the block, as (line within the block,
        // bytes back from that line's end) — the shape that survives a
        // prefix being added or stripped on the way through a container.
        let caret = self.caret.clamp(block.span.start, block.span.end);
        let block_line = self.source[block.span.start..caret].matches('\n').count();
        let line_end = self.source[caret..block.span.end]
            .find('\n')
            .map_or(block.span.end, |i| caret + i);
        let tail = line_end - caret;
        let block_lines = self.source[block.span.start..block.span.end]
            .matches('\n')
            .count()
            + 1;
        let upward = to <= block.span.start;
        self.record_caret();
        match self.editor.move_block(from, to) {
            Ok(_) if self.editor.source_str().ok().as_deref() == Some(self.source.as_str()) => {
                let _ = self.editor.undo();
                self.status = None;
                false
            }
            Ok(change) => {
                self.last_edit_kind = None; // structural edit is its own undo step
                self.refresh();
                let at = self.moved_block_caret(&change.new, upward, block_lines, block_line, tail);
                self.caret = at;
                self.anchor = None;
                self.goal_col = None;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.rebuild_map();
                self.clamp_caret();
                self.record_caret();
                true
            }
            // Nothing to move: the block dropped back onto its own boundary.
            Err(twig::Error::InvalidArgument) => {
                self.status = None;
                false
            }
            Err(e) => {
                self.status = Some(format!("move block: {e}"));
                false
            }
        }
    }

    /// The caret's place in the rewritten region after a move: the moved
    /// block's lines open the region when it went up (below the blank line it
    /// was dropped on, when it was) and close it (above the separator twig
    /// wrote after it) when it went down, and within them the
    /// caret keeps its line and its distance from that line's end. Clamped
    /// into the region rather than trusted, since a block can lose a line on
    /// the way — a quote it was the only content of goes with it.
    fn moved_block_caret(
        &self,
        new: &Range<usize>,
        upward: bool,
        block_lines: usize,
        block_line: usize,
        tail: usize,
    ) -> usize {
        let region = &self.source[new.start.min(self.source.len())..new.end.min(self.source.len())];
        let mut lines: Vec<(usize, usize)> = Vec::new();
        let mut start = 0;
        loop {
            match region[start..].find('\n') {
                Some(i) => {
                    lines.push((start, start + i));
                    start += i + 1;
                }
                None => {
                    lines.push((start, region.len()));
                    break;
                }
            }
        }
        // A separator line is blank, or a quote's bare `>`; the region can
        // open with one (the blank line the block was dropped on) or close
        // with one (the one twig wrote after it).
        let separator = |&(s, e): &(usize, usize)| {
            region[s..e]
                .trim()
                .trim_start_matches('>')
                .trim()
                .is_empty()
        };
        let first = if upward {
            lines.iter().position(|l| !separator(l)).unwrap_or(0)
        } else {
            let filled = lines
                .iter()
                .rposition(|l| !separator(l))
                .map_or(0, |i| i + 1);
            filled.saturating_sub(block_lines)
        };
        let (s, e) = lines[(first + block_line).min(lines.len() - 1)];
        new.start + e.saturating_sub(tail).max(s)
    }

    /// Move the caret's block one place up — Alt+↑: above the block before it,
    /// and out of its container, to just above it, when it is the first block
    /// there. Nothing above the document's first block, and nothing to do for
    /// a caret on a blank line; both say so in the status line.
    pub fn move_block_up(&mut self) {
        self.move_block_by(true);
    }

    /// Move the caret's block one place down — Alt+↓, the mirror of
    /// [`move_block_up`](Self::move_block_up): below the block after it, and
    /// out of its container when it is the last block there.
    pub fn move_block_down(&mut self) {
        self.move_block_by(false);
    }

    fn move_block_by(&mut self, up: bool) {
        if self.read_only || self.refuse_unsupported("move block", Gesture::MoveBlock) {
            return;
        }
        let Some(from) = self.block_offset_for_caret() else {
            self.status = Some("move block: no block here".into());
            return;
        };
        let Some(block) = self.movable_block_at(from) else {
            self.status = Some("move block: no block here".into());
            return;
        };
        let nodes = self.nodes();
        let to = if up {
            self.boundary_above(&nodes, &block)
        } else {
            self.boundary_below(&nodes, &block)
        };
        if to.is_none_or(|to| !self.move_block(from, to)) && self.status.is_none() {
            self.status = Some(if up {
                "move block: nothing above".into()
            } else {
                "move block: nothing below".into()
            });
        }
    }

    /// The boundary one step above `block`: before its previous sibling, or
    /// — for the first block in a container — before the container itself.
    /// `None` for the document's first block.
    fn boundary_above(&self, nodes: &[FlatNode], block: &FlatNode) -> Option<usize> {
        let parent = block.parent.and_then(|p| nodes.get(p.0 as usize))?;
        let previous = nodes
            .iter()
            .filter(|n| n.parent == Some(parent.id))
            .take_while(|n| n.id != block.id)
            .last();
        match previous {
            Some(p) => self.before(p),
            None if parent.kind == Kind::Doc => None,
            // An item leaving a nested list upward goes before the item
            // holding that list, as an item of the outer one; the list's own
            // first byte is inside the holding item's tail.
            None if matches!(
                parent.kind,
                Kind::BulletList | Kind::OrderedList | Kind::TaskList
            ) =>
            {
                match parent.parent.and_then(|p| nodes.get(p.0 as usize)) {
                    Some(item) if matches!(item.kind, Kind::ListItem | Kind::TaskListItem) => {
                        self.before(item)
                    }
                    _ => self.before(parent),
                }
            }
            None => self.before(parent),
        }
    }

    /// The boundary one step below `block`: after its next sibling, or — for
    /// the last block in a container — just past the container, at its
    /// parent's level ([`exit_below`](Self::exit_below)). `None` for the
    /// document's last block.
    fn boundary_below(&self, nodes: &[FlatNode], block: &FlatNode) -> Option<usize> {
        let parent = block.parent.and_then(|p| nodes.get(p.0 as usize))?;
        if let Some(n) = block.next_sibling.and_then(|n| nodes.get(n.0 as usize)) {
            return Some(self.after(n));
        }
        match parent.kind {
            Kind::Doc => None,
            _ => self.exit_below(nodes, parent),
        }
    }

    /// The boundary just past `container` at its parent's level: before its
    /// next sibling, or past its parent when it is the last thing there —
    /// the document's end at the top. Leaving a list item is the exception
    /// that keeps a block in the list: the next item's tail, which is what
    /// [`after`](Self::after) an item names.
    fn exit_below(&self, nodes: &[FlatNode], container: &FlatNode) -> Option<usize> {
        let item = matches!(container.kind, Kind::ListItem | Kind::TaskListItem);
        match container.next_sibling.and_then(|n| nodes.get(n.0 as usize)) {
            Some(n) if item => Some(self.after(n)),
            Some(n) => self.before(n),
            None => {
                let parent = container.parent.and_then(|p| nodes.get(p.0 as usize))?;
                match parent.kind {
                    Kind::Doc => Some(self.source.len()),
                    _ => self.exit_below(nodes, parent),
                }
            }
        }
    }

    /// The boundary before `node`: its first byte. twig reads a container's
    /// opening as before it — a quote's marker, a fence's first byte — so the
    /// span's start is the boundary at the parent's level for every kind.
    fn before(&self, node: &FlatNode) -> Option<usize> {
        Some(node.span.start)
    }

    /// The boundary after `node`: its own end, which for a container proper
    /// (a quote, a list, a fenced or tagged container) is the end of its last
    /// line — after its last block, still inside it, where a block arriving
    /// from outside joins it. Past the container is the next line's start: a
    /// blank line, the next block, or the document's end.
    fn after(&self, node: &FlatNode) -> usize {
        let container = is_block_container(&node.kind)
            && !matches!(node.kind, Kind::ListItem | Kind::TaskListItem);
        let end = node.span.end.min(self.source.len());
        if container && self.source.as_bytes().get(end) == Some(&b'\n') {
            end + 1
        } else {
            end
        }
    }

    /// Where a block dragged over rendered row `row` would land — the
    /// boundary before the row's block when the row is in its upper half, the
    /// boundary after it otherwise, and the document's end for a row below
    /// everything. `None` for a row that holds no block (a decoration row is
    /// resolved to the block under it, so this is a table's bottom rule with
    /// nothing after it, or a map with no rows). The frontend draws its
    /// indicator above [`DropTarget::row`] and hands
    /// [`DropTarget::offset`] to [`move_block`](Self::move_block).
    ///
    /// The block is the deepest one, not the widened item: a drop on a
    /// bullet's text lands before or after that bullet, and its nested
    /// children — rows of their own — answer for themselves.
    pub fn drop_target_at(&mut self, row: usize) -> Option<DropTarget> {
        let rows = self.vmap.rows.len();
        if row >= rows {
            return Some(DropTarget {
                offset: self.source.len(),
                row: rows,
            });
        }
        // A gap or a rule is not a block; the block under it is the one meant.
        let row = (row..rows).find(|&r| self.vmap.row_is_navigable(r))?;
        let start = self.vmap.row_start(row)?;
        let chain = self.editor.ancestors_at(start).ok()?;
        let block = chain
            .iter()
            .rev()
            .find(|m| !wysiwyg::is_inline_kind(&m.kind) && !is_block_container(&m.kind))?;
        let span = block.span.clone();
        let (first, last) = self.vmap.row_range_for(span.clone());
        if row <= usize::midpoint(first, last) {
            Some(DropTarget {
                offset: span.start,
                row: first,
            })
        } else {
            Some(DropTarget {
                offset: span.end.min(self.source.len()),
                row: last + 1,
            })
        }
    }
}
