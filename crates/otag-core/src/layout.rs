//! How a tab's panes divide the space they have.
//!
//! A tab holds a tree: a leaf is a pane, a split arranges its children left
//! to right or top to bottom. Each child carries a weight, and a split hands
//! out its space in proportion to those weights, which is what makes a
//! resize a change to two numbers rather than a recomputation of everything.
//!
//! Geometry lives on the daemon's side, together with the tree, because the
//! daemon is what tells each pane's program how large it is. A client is
//! handed finished rectangles, so two clients of the same session cannot
//! disagree about where a pane is.

use serde::{Deserialize, Serialize};

use crate::pane::{PaneId, PaneSize};

/// Space a split gives a child it has no other information about.
///
/// Splitting a pane halves its weight and gives the half to the new pane, so
/// the unit is a power of two: a pane can be split a few times before the
/// halves stop dividing evenly, and the proportions stay exact until then.
const WEIGHT: u32 = 1 << 10;

/// Columns the sidebar occupies when it is shown.
pub const SIDEBAR_COLS: u16 = 16;

/// Rows the tab bar occupies.
pub const TABS_ROWS: u16 = 1;

/// Width of the border otag draws around a pane.
pub const BORDER: u16 = 1;

/// How a split arranges its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Children sit next to one another, divided by a vertical line.
    LeftRight,
    /// Children sit above one another, divided by a horizontal line.
    TopBottom,
    /// A direction this build does not know about.
    #[serde(other)]
    Unknown,
}

/// A direction to look in: for the neighbouring pane, or for the edge to
/// move when resizing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Towards smaller columns.
    Left,
    /// Towards larger columns.
    Right,
    /// Towards smaller rows.
    Up,
    /// Towards larger rows.
    Down,
    /// A side this build does not know about.
    #[serde(other)]
    Unknown,
}

impl Side {
    /// The axis this side lies on.
    #[must_use]
    pub const fn axis(self) -> Direction {
        match self {
            Self::Left | Self::Right => Direction::LeftRight,
            Self::Up | Self::Down => Direction::TopBottom,
            Self::Unknown => Direction::Unknown,
        }
    }

    /// Whether this side is the one with the larger coordinates.
    #[must_use]
    pub const fn forward(self) -> bool {
        matches!(self, Self::Right | Self::Down)
    }
}

/// A rectangle of character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Area {
    /// Column of the left edge.
    pub x: u16,
    /// Row of the top edge.
    pub y: u16,
    /// Width in columns.
    pub cols: u16,
    /// Height in rows.
    pub rows: u16,
}

impl Area {
    /// A rectangle with no cells in it.
    pub const EMPTY: Self = Self {
        x: 0,
        y: 0,
        cols: 0,
        rows: 0,
    };

    /// A rectangle at `x`, `y` of the given size.
    #[must_use]
    pub const fn new(x: u16, y: u16, cols: u16, rows: u16) -> Self {
        Self { x, y, cols, rows }
    }

    /// The whole of a pane of this size, at the origin.
    #[must_use]
    pub const fn of(size: PaneSize) -> Self {
        Self::new(0, 0, size.cols(), size.rows())
    }

    /// Whether the rectangle has no cells.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.cols == 0 || self.rows == 0
    }

    /// Column just past the right edge.
    #[must_use]
    pub const fn right(self) -> u16 {
        self.x.saturating_add(self.cols)
    }

    /// Row just past the bottom edge.
    #[must_use]
    pub const fn bottom(self) -> u16 {
        self.y.saturating_add(self.rows)
    }

    /// Whether the cell at `x`, `y` is inside the rectangle.
    #[must_use]
    pub const fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The rectangle `by` cells in from every edge, empty when nothing is
    /// left.
    #[must_use]
    pub const fn inset(self, by: u16) -> Self {
        let twice = by.saturating_mul(2);
        if self.cols <= twice || self.rows <= twice {
            return Self::EMPTY;
        }
        Self::new(
            self.x.saturating_add(by),
            self.y.saturating_add(by),
            self.cols - twice,
            self.rows - twice,
        )
    }

    /// The rectangle as a pane size, which is never smaller than one cell.
    #[must_use]
    pub const fn size(self) -> PaneSize {
        PaneSize::new(self.rows, self.cols)
    }
}

/// Where otag's own furniture goes, and what is left for panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chrome {
    /// The workspace sidebar, empty when it is hidden.
    pub sidebar: Area,
    /// The tab bar above the panes.
    pub tabs: Area,
    /// Everything the layout tree may use.
    pub content: Area,
}

impl Chrome {
    /// Divide a client's terminal between otag's furniture and the panes.
    ///
    /// A terminal too small for the sidebar or the tab bar loses them rather
    /// than losing the panes: something has to give, and the panes are the
    /// part the user is working in.
    #[must_use]
    pub fn of(size: PaneSize, sidebar: bool) -> Self {
        let whole = Area::of(size);

        let sidebar_cols = if sidebar && whole.cols > SIDEBAR_COLS.saturating_mul(2) {
            SIDEBAR_COLS
        } else {
            0
        };
        let tabs_rows = if whole.rows > TABS_ROWS { TABS_ROWS } else { 0 };

        Self {
            sidebar: Area::new(whole.x, whole.y, sidebar_cols, whole.rows),
            tabs: Area::new(
                whole.x.saturating_add(sidebar_cols),
                whole.y,
                whole.cols.saturating_sub(sidebar_cols),
                tabs_rows,
            ),
            content: Area::new(
                whole.x.saturating_add(sidebar_cols),
                whole.y.saturating_add(tabs_rows),
                whole.cols.saturating_sub(sidebar_cols),
                whole.rows.saturating_sub(tabs_rows),
            ),
        }
    }
}

/// One child of a split: a share of the space, and what fills it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    /// This child's share of its split's space, relative to its siblings'.
    pub weight: u32,
    /// What fills the share.
    pub node: Node,
}

impl Branch {
    /// A child with the default share.
    fn new(node: Node) -> Self {
        Self {
            weight: WEIGHT,
            node,
        }
    }
}

/// A pane, or a split of further nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    /// One pane, filling the space it was given.
    Leaf {
        /// The pane that fills it.
        pane: PaneId,
    },
    /// Space divided between further nodes.
    Split {
        /// How the children are arranged.
        direction: Direction,
        /// The children, in the direction's order.
        children: Vec<Branch>,
    },
}

impl Node {
    /// Whether `pane` is this node or anywhere below it.
    fn contains(&self, pane: PaneId) -> bool {
        match self {
            Self::Leaf { pane: id } => *id == pane,
            Self::Split { children, .. } => children.iter().any(|child| child.node.contains(pane)),
        }
    }

    /// Append every pane below this node, in the order they are drawn.
    fn collect(&self, into: &mut Vec<PaneId>) {
        match self {
            Self::Leaf { pane } => into.push(*pane),
            Self::Split { children, .. } => {
                for child in children {
                    child.node.collect(into);
                }
            }
        }
    }

    /// Give `area` out to the panes below this node.
    fn areas(&self, area: Area, into: &mut Vec<(PaneId, Area)>) {
        match self {
            Self::Leaf { pane } => into.push((*pane, area)),
            Self::Split {
                direction,
                children,
            } => {
                for (child, part) in children.iter().zip(shares(*direction, area, children)) {
                    child.node.areas(part, into);
                }
            }
        }
    }
}

/// Divide `area` between `children` along `direction`.
///
/// Edges are computed from the running total of the weights rather than one
/// child at a time, so the parts never overlap and never leave a gap, however
/// the rounding falls.
fn shares(direction: Direction, area: Area, children: &[Branch]) -> Vec<Area> {
    let total: u64 = children.iter().map(|child| u64::from(child.weight)).sum();
    if total == 0 || children.is_empty() {
        return vec![Area::EMPTY; children.len()];
    }

    let length = u64::from(match direction {
        Direction::TopBottom => area.rows,
        // An unknown direction cannot be drawn in; treating it as columns
        // keeps the panes reachable instead of hiding them.
        Direction::LeftRight | Direction::Unknown => area.cols,
    });

    let mut parts = Vec::with_capacity(children.len());
    let mut done = 0_u64;
    let mut start = 0_u64;
    for child in children {
        done += u64::from(child.weight);
        let end = length * done / total;
        let span = u16::try_from(end.saturating_sub(start)).unwrap_or(u16::MAX);
        let offset = u16::try_from(start).unwrap_or(u16::MAX);

        parts.push(match direction {
            Direction::TopBottom => {
                Area::new(area.x, area.y.saturating_add(offset), area.cols, span)
            }
            Direction::LeftRight | Direction::Unknown => {
                Area::new(area.x.saturating_add(offset), area.y, span, area.rows)
            }
        });
        start = end;
    }
    parts
}

/// The panes of one tab and how they divide its space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    root: Node,
}

impl Layout {
    /// A layout holding one pane.
    #[must_use]
    pub const fn new(pane: PaneId) -> Self {
        Self {
            root: Node::Leaf { pane },
        }
    }

    /// The whole tree, for a snapshot to keep.
    #[must_use]
    pub const fn root(&self) -> &Node {
        &self.root
    }

    /// A layout rebuilt from a snapshot.
    #[must_use]
    pub const fn from_root(root: Node) -> Self {
        Self { root }
    }

    /// Every pane, in the order they are drawn.
    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        self.root.collect(&mut panes);
        panes
    }

    /// Whether the layout holds `pane`.
    #[must_use]
    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.contains(pane)
    }

    /// Where every pane is, given the space the tab has.
    #[must_use]
    pub fn areas(&self, area: Area) -> Vec<(PaneId, Area)> {
        let mut areas = Vec::new();
        self.root.areas(area, &mut areas);
        areas
    }

    /// Where one pane is, given the space the tab has.
    #[must_use]
    pub fn area_of(&self, pane: PaneId, area: Area) -> Option<Area> {
        self.areas(area)
            .into_iter()
            .find_map(|(id, part)| (id == pane).then_some(part))
    }

    /// Divide `at` in two and put `new_pane` in the second half.
    ///
    /// A split in the same direction as the one the pane already sits in
    /// becomes another child of it rather than a nested split, so that
    /// three panes side by side resize against one another rather than in
    /// pairs. Returns whether `at` was found.
    pub fn split(&mut self, at: PaneId, direction: Direction, new_pane: PaneId) -> bool {
        split_in(&mut self.root, at, direction, new_pane)
    }

    /// Remove `pane`, giving its space to the pane it was split from.
    ///
    /// Returns whether the layout still holds a pane. One that does not is
    /// not a layout any more — a tree has no way to hold nothing — and
    /// belongs to a tab its owner has to drop.
    pub fn close(&mut self, pane: PaneId) -> bool {
        if !self.contains(pane) {
            return true;
        }
        // The root being emptied means the pane that closed was the last one.
        !close_in(&mut self.root, pane)
    }

    /// The pane next to `pane` on `side`, if there is one.
    ///
    /// Found geometrically rather than from the tree, so that the pane the
    /// user reaches is the one they see in that direction even when the two
    /// are in different branches of the tree.
    #[must_use]
    pub fn neighbour(&self, pane: PaneId, side: Side, area: Area) -> Option<PaneId> {
        let areas = self.areas(area);
        let from = areas
            .iter()
            .find_map(|(id, part)| (*id == pane).then_some(*part))?;

        areas
            .iter()
            .filter(|(id, _)| *id != pane)
            .filter_map(|(id, part)| {
                // A candidate has to be on the right side of the pane, and
                // to overlap it across that side, so that moving left from a
                // tall pane does not jump to something above or below it.
                let distance = match side {
                    Side::Left => (part.right() <= from.x && overlaps_rows(*part, from))
                        .then(|| from.x.saturating_sub(part.right())),
                    Side::Right => (part.x >= from.right() && overlaps_rows(*part, from))
                        .then(|| part.x.saturating_sub(from.right())),
                    Side::Up => (part.bottom() <= from.y && overlaps_cols(*part, from))
                        .then(|| from.y.saturating_sub(part.bottom())),
                    Side::Down => (part.y >= from.bottom() && overlaps_cols(*part, from))
                        .then(|| part.y.saturating_sub(from.bottom())),
                    Side::Unknown => None,
                }?;
                // Among equally close candidates, the one nearest the pane's
                // own top-left corner wins, so that moving right from a tall
                // pane lands on the topmost of the panes beside it rather
                // than on whichever one happens to straddle its middle.
                let offset = match side {
                    Side::Left | Side::Right => part.y.abs_diff(from.y),
                    Side::Up | Side::Down => part.x.abs_diff(from.x),
                    Side::Unknown => 0,
                };
                Some((distance, offset, *id))
            })
            .min()
            .map(|(_, _, id)| id)
    }

    /// Move the edge of `pane` on `side` outwards by `amount` cells' worth
    /// of weight, taking the space from the neighbour behind that edge.
    ///
    /// Returns whether anything moved.
    pub fn resize(&mut self, pane: PaneId, side: Side, amount: u32) -> bool {
        if matches!(side, Side::Unknown) || amount == 0 {
            return false;
        }
        resize_in(&mut self.root, pane, side.axis(), side.forward(), amount)
    }
}

/// Whether two rectangles share a row.
fn overlaps_rows(left: Area, right: Area) -> bool {
    left.y < right.bottom() && right.y < left.bottom()
}

/// Whether two rectangles share a column.
fn overlaps_cols(left: Area, right: Area) -> bool {
    left.x < right.right() && right.x < left.right()
}

/// Split one pane inside `node`. Returns whether the pane was found.
fn split_in(node: &mut Node, at: PaneId, direction: Direction, new_pane: PaneId) -> bool {
    match node {
        Node::Leaf { pane } if *pane == at => {
            *node = Node::Split {
                direction,
                children: vec![
                    Branch::new(Node::Leaf { pane: at }),
                    Branch::new(Node::Leaf { pane: new_pane }),
                ],
            };
            true
        }
        Node::Leaf { .. } => false,
        Node::Split {
            direction: own,
            children,
        } => {
            if *own == direction
                && let Some(index) = children
                    .iter()
                    .position(|child| matches!(child.node, Node::Leaf { pane } if pane == at))
            {
                // The new pane takes half of what the old one had, so the
                // other panes keep exactly the space they had before.
                let weight = children[index].weight.max(2);
                let kept = weight / 2;
                children[index].weight = kept;
                children.insert(
                    index + 1,
                    Branch {
                        weight: weight - kept,
                        node: Node::Leaf { pane: new_pane },
                    },
                );
                return true;
            }
            children
                .iter_mut()
                .any(|child| split_in(&mut child.node, at, direction, new_pane))
        }
    }
}

/// Remove a pane from `node`. Returns whether `node` itself is now empty and
/// has to be removed by its parent.
fn close_in(node: &mut Node, pane: PaneId) -> bool {
    match node {
        Node::Leaf { pane: id } => *id == pane,
        Node::Split { children, .. } => {
            let mut emptied = None;
            for (index, child) in children.iter_mut().enumerate() {
                if close_in(&mut child.node, pane) {
                    emptied = Some(index);
                    break;
                }
            }
            let Some(index) = emptied else {
                return false;
            };

            // The space the pane had goes to the pane it was split from,
            // which is the nearest sibling.
            let removed = children.remove(index);
            let neighbour = index.min(children.len().saturating_sub(1));
            if let Some(child) = children.get_mut(neighbour) {
                child.weight = child.weight.saturating_add(removed.weight);
            }

            match children.len() {
                0 => true,
                // A split with one child is the child; collapsing it keeps
                // the tree the shape the user sees.
                1 => {
                    let only = children.remove(0);
                    *node = only.node;
                    false
                }
                _ => false,
            }
        }
    }
}

/// Move one edge inside `node`. Returns whether anything moved.
fn resize_in(node: &mut Node, pane: PaneId, axis: Direction, forward: bool, amount: u32) -> bool {
    let Node::Split {
        direction,
        children,
    } = node
    else {
        return false;
    };

    // The deepest split that can take the change is the one whose divider
    // the user sees next to the pane.
    if children
        .iter_mut()
        .any(|child| resize_in(&mut child.node, pane, axis, forward, amount))
    {
        return true;
    }
    if *direction != axis {
        return false;
    }

    let Some(index) = children.iter().position(|child| child.node.contains(pane)) else {
        return false;
    };
    let partner = if forward {
        index.checked_add(1)
    } else {
        index.checked_sub(1)
    };
    let Some(partner) = partner.filter(|partner| *partner < children.len()) else {
        return false;
    };

    // The neighbour keeps at least a sliver, so a pane cannot be resized
    // out of existence.
    let available = children[partner].weight.saturating_sub(1);
    let moved = amount.min(available);
    if moved == 0 {
        return false;
    }
    children[partner].weight -= moved;
    children[index].weight = children[index].weight.saturating_add(moved);
    true
}

/// Weight worth one cell of an area that long, for turning a key press into
/// a resize the user can see.
#[must_use]
pub const fn weight_per_cell(cells: u16) -> u32 {
    if cells == 0 {
        return WEIGHT;
    }
    let per = WEIGHT / cells as u32;
    if per == 0 { 1 } else { per }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: u64) -> PaneId {
        let mut pane = PaneId::FIRST;
        for _ in 1..id {
            pane = pane.next();
        }
        pane
    }

    fn layout_of(ids: &[u64], direction: Direction) -> Layout {
        let mut layout = Layout::new(pane(ids[0]));
        for (index, id) in ids.iter().enumerate().skip(1) {
            assert!(layout.split(pane(ids[index - 1]), direction, pane(*id)));
        }
        layout
    }

    #[test]
    fn a_new_layout_holds_one_pane_filling_everything() {
        let layout = Layout::new(pane(1));
        let area = Area::new(0, 0, 40, 10);
        assert_eq!(layout.areas(area), vec![(pane(1), area)]);
        assert_eq!(layout.panes(), vec![pane(1)]);
    }

    #[test]
    fn a_split_divides_the_space_in_two() {
        let mut layout = Layout::new(pane(1));
        assert!(layout.split(pane(1), Direction::LeftRight, pane(2)));

        let areas = layout.areas(Area::new(0, 0, 40, 10));
        assert_eq!(
            areas,
            vec![
                (pane(1), Area::new(0, 0, 20, 10)),
                (pane(2), Area::new(20, 0, 20, 10)),
            ]
        );
    }

    #[test]
    fn a_top_to_bottom_split_divides_the_rows() {
        let mut layout = Layout::new(pane(1));
        assert!(layout.split(pane(1), Direction::TopBottom, pane(2)));

        let areas = layout.areas(Area::new(2, 3, 40, 10));
        assert_eq!(
            areas,
            vec![
                (pane(1), Area::new(2, 3, 40, 5)),
                (pane(2), Area::new(2, 8, 40, 5)),
            ]
        );
    }

    #[test]
    fn an_odd_number_of_cells_is_handed_out_without_a_gap() {
        let layout = layout_of(&[1, 2, 3], Direction::LeftRight);
        let areas = layout.areas(Area::new(0, 0, 10, 4));

        // Whatever the rounding, the parts touch and together fill the area.
        let mut edge = 0;
        for (_, part) in &areas {
            assert_eq!(part.x, edge);
            edge = part.right();
        }
        assert_eq!(edge, 10);
    }

    #[test]
    fn a_split_in_the_same_direction_becomes_a_sibling_not_a_nested_split() {
        // Three panes side by side resize against one another, which they
        // would not if the third were nested inside the second.
        let layout = layout_of(&[1, 2, 3], Direction::LeftRight);
        let Node::Split { children, .. } = layout.root() else {
            panic!("expected a split");
        };
        assert_eq!(children.len(), 3);
    }

    #[test]
    fn a_split_in_the_other_direction_nests() {
        let mut layout = Layout::new(pane(1));
        assert!(layout.split(pane(1), Direction::LeftRight, pane(2)));
        assert!(layout.split(pane(2), Direction::TopBottom, pane(3)));

        let areas = layout.areas(Area::new(0, 0, 40, 10));
        assert_eq!(
            areas,
            vec![
                (pane(1), Area::new(0, 0, 20, 10)),
                (pane(2), Area::new(20, 0, 20, 5)),
                (pane(3), Area::new(20, 5, 20, 5)),
            ]
        );
    }

    #[test]
    fn splitting_a_pane_the_layout_does_not_have_changes_nothing() {
        let mut layout = Layout::new(pane(1));
        assert!(!layout.split(pane(9), Direction::LeftRight, pane(2)));
        assert_eq!(layout.panes(), vec![pane(1)]);
    }

    #[test]
    fn splitting_only_takes_space_from_the_pane_that_was_split() {
        let layout = layout_of(&[1, 2], Direction::LeftRight);
        let before = layout.areas(Area::new(0, 0, 40, 10));

        let mut after = layout.clone();
        assert!(after.split(pane(2), Direction::LeftRight, pane(3)));
        let after = after.areas(Area::new(0, 0, 40, 10));

        assert_eq!(after[0].1, before[0].1, "the untouched pane keeps its size");
        assert_eq!(after[1].1.cols + after[2].1.cols, before[1].1.cols);
    }

    #[test]
    fn closing_a_pane_gives_its_space_to_its_neighbour() {
        let mut layout = layout_of(&[1, 2], Direction::LeftRight);
        assert!(layout.close(pane(2)));

        assert_eq!(layout.panes(), vec![pane(1)]);
        let area = Area::new(0, 0, 40, 10);
        assert_eq!(layout.areas(area), vec![(pane(1), area)]);
    }

    #[test]
    fn closing_the_last_pane_says_the_layout_is_over() {
        // There is no such thing as a layout with no panes, so the answer is
        // the signal for its owner to drop the tab it belongs to.
        let mut layout = Layout::new(pane(1));
        assert!(!layout.close(pane(1)));
    }

    #[test]
    fn closing_a_pane_collapses_the_split_it_leaves_behind() {
        let mut layout = Layout::new(pane(1));
        assert!(layout.split(pane(1), Direction::LeftRight, pane(2)));
        assert!(layout.split(pane(2), Direction::TopBottom, pane(3)));
        assert!(layout.close(pane(3)));

        // The nested split is gone, so pane 2 fills the half it shares.
        let areas = layout.areas(Area::new(0, 0, 40, 10));
        assert_eq!(
            areas,
            vec![
                (pane(1), Area::new(0, 0, 20, 10)),
                (pane(2), Area::new(20, 0, 20, 10)),
            ]
        );
    }

    #[test]
    fn closing_a_pane_the_layout_does_not_have_leaves_it_alone() {
        let mut layout = layout_of(&[1, 2], Direction::LeftRight);
        assert!(layout.close(pane(9)));
        assert_eq!(layout.panes(), vec![pane(1), pane(2)]);
    }

    #[test]
    fn the_neighbour_is_the_pane_on_that_side() {
        let layout = layout_of(&[1, 2, 3], Direction::LeftRight);
        let area = Area::new(0, 0, 30, 10);

        assert_eq!(layout.neighbour(pane(2), Side::Left, area), Some(pane(1)));
        assert_eq!(layout.neighbour(pane(2), Side::Right, area), Some(pane(3)));
        assert_eq!(layout.neighbour(pane(1), Side::Left, area), None);
        assert_eq!(layout.neighbour(pane(3), Side::Right, area), None);
        assert_eq!(layout.neighbour(pane(2), Side::Up, area), None);
    }

    #[test]
    fn the_neighbour_is_found_across_branches_of_the_tree() {
        // Pane 1 on the left, panes 2 and 3 stacked on the right. Moving
        // right from pane 1 reaches the pane it is level with.
        let mut layout = Layout::new(pane(1));
        assert!(layout.split(pane(1), Direction::LeftRight, pane(2)));
        assert!(layout.split(pane(2), Direction::TopBottom, pane(3)));
        let area = Area::new(0, 0, 40, 10);

        assert_eq!(layout.neighbour(pane(1), Side::Right, area), Some(pane(2)));
        assert_eq!(layout.neighbour(pane(3), Side::Left, area), Some(pane(1)));
        assert_eq!(layout.neighbour(pane(2), Side::Down, area), Some(pane(3)));
        assert_eq!(layout.neighbour(pane(3), Side::Up, area), Some(pane(2)));
    }

    #[test]
    fn a_pane_the_layout_does_not_have_has_no_neighbour() {
        let layout = layout_of(&[1, 2], Direction::LeftRight);
        assert_eq!(
            layout.neighbour(pane(9), Side::Left, Area::new(0, 0, 20, 5)),
            None
        );
    }

    #[test]
    fn resizing_moves_the_divider_and_leaves_the_total_alone() {
        let mut layout = layout_of(&[1, 2], Direction::LeftRight);
        let area = Area::new(0, 0, 40, 10);
        let before = layout.areas(area);

        assert!(layout.resize(pane(1), Side::Right, weight_per_cell(40) * 4));
        let after = layout.areas(area);

        assert!(
            after[0].1.cols > before[0].1.cols,
            "the pane grew: {before:?} -> {after:?}"
        );
        assert_eq!(after[0].1.cols + after[1].1.cols, 40);
        assert_eq!(after[1].1.x, after[0].1.right());
    }

    #[test]
    fn resizing_the_other_way_shrinks_the_pane() {
        let mut layout = layout_of(&[1, 2], Direction::LeftRight);
        let area = Area::new(0, 0, 40, 10);

        assert!(layout.resize(pane(2), Side::Left, weight_per_cell(40) * 4));
        let after = layout.areas(area);
        assert!(after[1].1.cols > after[0].1.cols);
    }

    #[test]
    fn resizing_across_the_wrong_axis_does_nothing() {
        let mut layout = layout_of(&[1, 2], Direction::LeftRight);
        assert!(!layout.resize(pane(1), Side::Down, WEIGHT));
    }

    #[test]
    fn a_resize_cannot_squeeze_a_neighbour_out_of_existence() {
        let mut layout = layout_of(&[1, 2], Direction::LeftRight);
        assert!(layout.resize(pane(1), Side::Right, u32::MAX));
        // The neighbour keeps a sliver, so it can be grown back.
        assert!(layout.resize(pane(2), Side::Left, WEIGHT));
        assert_eq!(layout.panes(), vec![pane(1), pane(2)]);
    }

    #[test]
    fn a_resize_without_a_neighbour_on_that_side_does_nothing() {
        let mut layout = layout_of(&[1, 2], Direction::LeftRight);
        assert!(!layout.resize(pane(1), Side::Left, WEIGHT));
        assert!(!layout.resize(pane(9), Side::Right, WEIGHT));
        assert!(!layout.resize(pane(1), Side::Unknown, WEIGHT));
        assert!(!layout.resize(pane(1), Side::Right, 0));
    }

    #[test]
    fn a_resize_moves_the_divider_nearest_the_pane() {
        // Panes 2 and 3 are stacked inside the right half. Growing pane 2
        // downwards moves the divider it shares with pane 3, not the outer
        // one it shares with pane 1.
        let mut layout = Layout::new(pane(1));
        assert!(layout.split(pane(1), Direction::LeftRight, pane(2)));
        assert!(layout.split(pane(2), Direction::TopBottom, pane(3)));
        let area = Area::new(0, 0, 40, 20);

        assert!(layout.resize(pane(2), Side::Down, weight_per_cell(20) * 4));
        let after = layout.areas(area);
        assert_eq!(after[0].1.cols, 20, "the left pane is untouched");
        assert!(after[1].1.rows > after[2].1.rows);
    }

    #[test]
    fn an_area_insets_by_its_border_and_never_underflows() {
        assert_eq!(
            Area::new(2, 3, 10, 6).inset(1),
            Area::new(3, 4, 8, 4),
            "a border takes one cell from every edge"
        );
        assert_eq!(Area::new(0, 0, 2, 2).inset(1), Area::EMPTY);
        assert_eq!(Area::new(0, 0, 1, 10).inset(1), Area::EMPTY);
        assert!(Area::EMPTY.inset(1).is_empty());
    }

    #[test]
    fn an_area_knows_what_it_contains() {
        let area = Area::new(2, 3, 4, 5);
        assert!(area.contains(2, 3));
        assert!(area.contains(5, 7));
        assert!(!area.contains(6, 7));
        assert!(!area.contains(2, 8));
        assert!(!area.contains(1, 3));
    }

    #[test]
    fn an_empty_area_still_has_a_usable_pane_size() {
        // A pseudo-terminal cannot be zero cells; a pane squeezed to nothing
        // is told it is one cell rather than failing to be resized at all.
        assert_eq!(Area::EMPTY.size(), PaneSize::new(1, 1));
    }

    #[test]
    fn the_chrome_leaves_the_rest_to_the_panes() {
        let chrome = Chrome::of(PaneSize::new(24, 80), true);
        assert_eq!(chrome.sidebar, Area::new(0, 0, SIDEBAR_COLS, 24));
        assert_eq!(
            chrome.tabs,
            Area::new(SIDEBAR_COLS, 0, 80 - SIDEBAR_COLS, 1)
        );
        assert_eq!(
            chrome.content,
            Area::new(SIDEBAR_COLS, 1, 80 - SIDEBAR_COLS, 23)
        );
    }

    #[test]
    fn hiding_the_sidebar_gives_its_columns_to_the_panes() {
        let chrome = Chrome::of(PaneSize::new(24, 80), false);
        assert!(chrome.sidebar.is_empty());
        assert_eq!(chrome.content, Area::new(0, 1, 80, 23));
    }

    #[test]
    fn a_terminal_too_narrow_for_the_sidebar_loses_it_rather_than_the_panes() {
        let chrome = Chrome::of(PaneSize::new(10, 20), true);
        assert!(chrome.sidebar.is_empty());
        assert_eq!(chrome.content.cols, 20);
    }

    #[test]
    fn a_terminal_one_row_tall_keeps_the_row_for_the_panes() {
        let chrome = Chrome::of(PaneSize::new(1, 80), true);
        assert!(chrome.tabs.is_empty());
        assert_eq!(chrome.content.rows, 1);
    }

    #[test]
    fn an_unknown_direction_still_hands_out_space() {
        // A layout from a newer otag must stay usable rather than hide its
        // panes.
        let mut layout = Layout::new(pane(1));
        assert!(layout.split(pane(1), Direction::Unknown, pane(2)));
        let areas = layout.areas(Area::new(0, 0, 10, 4));
        assert_eq!(areas.len(), 2);
        assert!(areas.iter().all(|(_, part)| !part.is_empty()));
    }

    #[test]
    fn weight_per_cell_is_never_zero() {
        assert!(weight_per_cell(0) > 0);
        assert!(weight_per_cell(1) > 0);
        assert!(weight_per_cell(u16::MAX) > 0);
    }

    #[test]
    fn a_layout_survives_a_roundtrip_through_json() {
        // A snapshot has to bring the tree back exactly, or a restored
        // session would not look like the one that was saved.
        let layout = layout_of(&[1, 2, 3], Direction::TopBottom);
        let json = serde_json::to_string(&layout).expect("a layout encodes");
        let back: Layout = serde_json::from_str(&json).expect("a layout decodes");
        assert_eq!(back, layout);
    }
}
